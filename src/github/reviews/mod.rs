mod evidence;
pub mod model;
mod presentation;
pub mod repair;

use std::collections::BTreeSet;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use self::model::{INSTRUCTIONS, model_review};
use crate::Result;
use crate::agent::Harness;
use crate::github::GitHub;

pub(crate) use evidence::allowed_dependency_path;
pub use evidence::{
    DependabotMetadata, checks, ci_blockers, compatibility, files_context, resolve,
};
pub use presentation::{decision, render};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewOutcome {
    pub approved: bool,
    pub published: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn review_pr(
    github: &GitHub,
    number: u64,
    required: &[String],
    model: Option<&str>,
    bot_slug: &str,
    harness: Harness,
    repository_private: Option<bool>,
    update_type: &str,
    maintainer_changes: &str,
    expected_head: &str,
    wait: Duration,
) -> Result<ReviewOutcome> {
    let slug = Regex::new(r"^[a-z0-9-]+$")?;
    if !slug.is_match(bot_slug)
        || required
            .iter()
            .any(|name| name.trim().is_empty() || name.starts_with("Pekin dependasolve"))
    {
        bail!("selected App slug and valid external required checks are required");
    }
    let (pull, dependency, metadata) = resolve(github, number)?;
    if let Some(metadata) = metadata {
        if update_type != metadata.update_type || maintainer_changes != metadata.maintainer_changes
        {
            bail!("Dependabot update metadata changed; rerun on the verified pull request state");
        }
    }
    let head = text(&pull, &["head", "sha"])?;
    if !expected_head.is_empty() && head != expected_head {
        bail!("the PR changed since this workflow started; rerun on the new commit");
    }
    let changed_files = pull["changed_files"].as_u64().unwrap_or_default() as usize;
    let (files, complete) = files_context(github, number, changed_files)?;
    let base_ref = text(&pull, &["base", "ref"])?;
    let endpoint = format!("branches/{}/protection", percent_encode(base_ref));
    let mut protection = github.api_optional(&endpoint, None, "GET")?;
    let mut rows = checks(github, head)?;
    let deadline = Instant::now() + wait;
    let names: BTreeSet<_> = required.iter().map(String::as_str).collect();
    while Instant::now() < deadline
        && (names
            .iter()
            .any(|name| !rows.iter().any(|row| row["name"] == **name))
            || rows.iter().any(|row| {
                names.contains(row["name"].as_str().unwrap_or_default())
                    && matches!(
                        row["state"].as_str(),
                        Some("queued" | "in_progress" | "pending" | "waiting" | "requested")
                    )
            }))
    {
        thread::sleep(
            Duration::from_secs(10).min(deadline.saturating_duration_since(Instant::now())),
        );
        rows = checks(github, head)?;
    }
    let context = json!({
        "number": number, "url": pull["html_url"],
        "head": head, "base": pull["base"]["sha"],
        "title": pull["title"].as_str().unwrap_or_default().chars().take(2000).collect::<String>(),
        "description": pull["body"].as_str().unwrap_or_default().chars().take(12000).collect::<String>(),
        "dependency": dependency, "files": files, "complete_diff": complete,
        "checks": rows, "update_type": update_type,
        "maintainer_changes": maintainer_changes,
    });
    let fingerprint = Sha256::digest(serde_json::to_vec(&json!([
        context,
        required,
        protection,
        harness.as_str(),
        model,
        INSTRUCTIONS
    ]))?);
    let marker = format!("<!-- pekin-review-{} -->", hex(&fingerprint[..12]));
    let reviews = github.pages(&format!("pulls/{number}/reviews"), None)?;
    let login = format!("{bot_slug}[bot]");
    let own: Vec<_> = reviews
        .iter()
        .filter(|item| item["user"]["login"] == login)
        .collect();
    if let Some(previous) = own.iter().find(|item| {
        item["body"]
            .as_str()
            .is_some_and(|body| body.contains(&marker))
            && item["commit_id"] == context["head"]
            && item["state"] != "DISMISSED"
    }) {
        println!("This commit and CI state already have a review");
        let approved = previous["state"] == "APPROVED";
        return Ok(ReviewOutcome {
            approved,
            published: false,
        });
    }
    let replacement = (|| -> Result<_> {
        let result = model_review(&context, model, harness, repository_private)?;
        let (current, _, _) = resolve(github, number)?;
        if current["head"]["sha"] != context["head"] || current["base"]["sha"] != context["base"] {
            bail!("the PR changed during review; rerun on the new commit");
        }
        if json!(checks(github, head)?) != context["checks"] {
            bail!("CI changed during review; rerun to assess the latest results");
        }
        protection = github.api_optional(&endpoint, None, "GET")?;
        let (event, blockers) = decision(&result, &context, required, protection.as_ref())?;
        let body = render(&result, &context, event, &blockers, &marker);
        Ok((event, body))
    })();
    let (event, body) = dismiss_before_publish(replacement, || {
        for previous in own.into_iter().filter(|item| item["state"] == "APPROVED") {
            github.api(
                &format!("pulls/{number}/reviews/{}/dismissals", previous["id"]),
                Some(&json!({"message": "Rechecking the current diff and CI results"})),
                "PUT",
            )?;
        }
        Ok(())
    })?;
    github.api(
        &format!("pulls/{number}/reviews"),
        Some(&json!({"commit_id": head, "event": event, "body": body})),
        "POST",
    )?;
    println!(
        "Published {} for {}",
        event.to_ascii_lowercase(),
        head.chars().take(12).collect::<String>()
    );
    let approved = event == "APPROVE";
    Ok(ReviewOutcome {
        approved,
        published: true,
    })
}

fn dismiss_before_publish<T>(
    replacement: Result<T>,
    dismiss: impl FnOnce() -> Result<()>,
) -> Result<T> {
    let replacement = replacement?;
    dismiss()?;
    Ok(replacement)
}

fn text<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str> {
    path.iter()
        .try_fold(value, |current, key| {
            current.get(*key).ok_or_else(|| anyhow!("missing {key}"))
        })?
        .as_str()
        .ok_or_else(|| anyhow!("expected text at {}", path.join(".")))
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::{dismiss_before_publish, percent_encode};

    #[test]
    fn percent_encoding_is_path_safe() {
        assert_eq!(percent_encode("feature/a b"), "feature%2Fa%20b");
    }

    #[test]
    fn failed_review_never_dismisses_an_existing_approval() {
        let mut dismissed = false;
        let outcome =
            dismiss_before_publish::<()>(Err(anyhow!("hosted models unavailable")), || {
                dismissed = true;
                Ok(())
            });

        assert!(outcome.is_err());
        assert!(!dismissed);
    }
}
