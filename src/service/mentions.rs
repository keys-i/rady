use std::collections::{BTreeMap, BTreeSet};
use std::env;

use anyhow::{anyhow, bail};
use serde_json::Value;

use crate::Result;
use crate::github::{self, GitHub};
use crate::setup;

use super::{ServeArgs, owner_matches, record_sweep_failure, validate_owner_filter};

const MAX_MENTION_CURSORS: usize = 512;

pub(super) fn sweep_with_token(
    arguments: &ServeArgs,
    token: &str,
    cursors: &mut BTreeMap<String, u64>,
) -> Result<()> {
    validate_owner_filter(arguments.owner.as_deref())?;
    let repositories =
        github::authenticated_pages("installation/repositories", "repositories", token)?;
    let model = env::var("RADY_MODEL")
        .ok()
        .filter(|value| !value.is_empty());
    let mut failures = Vec::new();
    bound_cursors(cursors);
    let mut repositories = repositories.iter().collect::<Vec<_>>();
    repositories.sort_by_key(|repository| repository["full_name"].as_str().unwrap_or_default());
    for repository in repositories {
        if repository["archived"].as_bool() == Some(true)
            || repository["disabled"].as_bool() == Some(true)
        {
            continue;
        }
        let Some(name) = repository["full_name"].as_str().filter(|name| {
            owner_matches(repository, arguments.owner.as_deref())
                && github::validate_repository(name).is_ok()
        }) else {
            continue;
        };
        let private = repository["private"].as_bool();
        let github = GitHub::new(name, token)?;
        let accepted = match github
            .raw_optional("contents/.github/rady.json")?
            .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        {
            Some(configuration) => match setup::verified_configuration(&github, &configuration) {
                Ok(accepted) => accepted,
                Err(error) => {
                    record_sweep_failure(&mut failures, name, &error);
                    continue;
                }
            },
            None => false,
        };
        if !accepted {
            continue;
        }
        let comments = match github.pages_after_id(
            "issues/comments?sort=created&direction=desc",
            cursors.get(name).copied(),
        ) {
            Ok(value) => value,
            Err(error) => {
                record_sweep_failure(&mut failures, name, &error);
                continue;
            }
        };
        let mut repo_failed = false;
        for comment in &comments {
            let Some(body) = comment["body"].as_str().filter(|body| {
                matches!(
                    comment["author_association"].as_str(),
                    Some("OWNER" | "MEMBER" | "COLLABORATOR")
                ) && crate::mentions::is_invocation(body)
            }) else {
                continue;
            };
            let (Some(comment_id), Some(issue)) = (
                comment["id"].as_u64(),
                comment_issue(name, comment["issue_url"].as_str().unwrap_or_default()),
            ) else {
                record_sweep_failure(
                    &mut failures,
                    name,
                    &anyhow!("GitHub returned an invalid mention comment"),
                );
                repo_failed = true;
                continue;
            };
            let _ = body;
            if let Err(error) = crate::mentions::respond_for_repository(
                &github,
                issue,
                comment_id,
                model.as_deref(),
                arguments.harness,
                private,
            ) {
                if crate::mentions::is_hosted_unavailable(&error) {
                    eprintln!(
                        "{name}: model providers are cooling down; Rady will retry these mentions next pass"
                    );
                    repo_failed = true;
                    break;
                }
                record_sweep_failure(&mut failures, name, &error);
                repo_failed = true;
            }
        }
        if !repo_failed {
            if let Some(last) = comments
                .iter()
                .filter_map(|comment| comment["id"].as_u64())
                .max()
            {
                remember_cursor(cursors, name, last);
            }
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    let noun = if failures.len() == 1 {
        "problem"
    } else {
        "problems"
    };
    bail!(
        "the mention pass hit {} {noun}: {}",
        failures.len(),
        failures.join("; ")
    )
}

fn remember_cursor(cursors: &mut BTreeMap<String, u64>, repository: &str, comment: u64) {
    cursors.insert(repository.to_owned(), comment);
    bound_cursors(cursors);
}

fn bound_cursors(cursors: &mut BTreeMap<String, u64>) {
    if cursors.len() <= MAX_MENTION_CURSORS {
        return;
    }
    let mut newest = cursors
        .iter()
        .map(|(repository, comment)| (repository.clone(), *comment))
        .collect::<Vec<_>>();
    newest.sort_unstable_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    newest.truncate(MAX_MENTION_CURSORS);
    let keep = newest
        .into_iter()
        .map(|(repository, _)| repository)
        .collect::<BTreeSet<_>>();
    cursors.retain(|repository, _| keep.contains(repository));
}

fn comment_issue(repo: &str, issue_url: &str) -> Option<u64> {
    let prefix = format!("https://api.github.com/repos/{repo}/issues/");
    issue_url
        .strip_prefix(&prefix)?
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn central_sweep_accepts_only_canonical_issue_urls() {
        for (repo, url, expected) in [
            (
                "keys-i/rady",
                "https://api.github.com/repos/keys-i/rady/issues/42",
                Some(42),
            ),
            (
                "keys-i/rady",
                "https://api.github.com/repos/other/rady/issues/42",
                None,
            ),
            (
                "keys-i/rady",
                "https://api.github.com/repos/keys-i/rady/issues/0",
                None,
            ),
            ("keys-i/rady", "not-a-url", None),
        ] {
            assert_eq!(comment_issue(repo, url), expected, "{url}");
        }
    }

    #[test]
    fn cursor_memory_survives_another_installation_and_has_a_recent_window_fallback() {
        let mut cursors = BTreeMap::from([
            ("first/repository".to_owned(), 3_100),
            ("second/repository".to_owned(), 3_099),
        ]);
        bound_cursors(&mut cursors);
        assert_eq!(cursors.get("first/repository"), Some(&3_100));

        for comment in 1..=(MAX_MENTION_CURSORS as u64 + 1) {
            remember_cursor(
                &mut cursors,
                &format!("owner/repository-{comment}"),
                comment,
            );
        }

        assert_eq!(cursors.len(), MAX_MENTION_CURSORS);
        assert_eq!(cursors.get("first/repository"), Some(&3_100));
        assert!(!cursors.contains_key("owner/repository-1"));
    }
}
