use std::collections::{BTreeMap, BTreeSet};
use std::env;

use anyhow::{anyhow, bail};
use serde_json::Value;

use crate::Result;
use crate::github::{self, GitHub};
use crate::setup;

use super::{SweepArgs, owner_matches, record_sweep_failure, validate_owner_filter};

pub(super) fn sweep(arguments: SweepArgs) -> Result<()> {
    let token = env::var("GH_TOKEN").unwrap_or_default();
    sweep_with_token(&arguments, &token, &mut BTreeMap::new())
}

pub(super) fn sweep_with_token(
    arguments: &SweepArgs,
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
    let visible = repositories
        .iter()
        .filter_map(|repository| repository["full_name"].as_str())
        .filter(|name| github::validate_repository(name).is_ok())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    cursors.retain(|name, _| visible.contains(name));
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
                cursors.insert(name.to_owned(), last);
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
}
