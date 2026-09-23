use std::env;
use std::time::Duration;

use anyhow::anyhow;
use serde_json::Value;

use crate::Result;
use crate::github::{self, GitHub};
use crate::reviews;
use crate::setup;

use super::{ServeArgs, ServiceCycleOutcome, owner_matches, record_sweep_failure};

pub(super) fn service_reviews(
    arguments: &ServeArgs,
    token: &str,
    max_reviews: usize,
) -> Result<ServiceCycleOutcome> {
    let repositories =
        github::authenticated_pages("installation/repositories", "repositories", token)?;
    let model = env::var("RADY_MODEL")
        .ok()
        .filter(|value| !value.is_empty());
    let slug = env::var("RADY_APP_SLUG")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "radyybot".to_owned());
    let mut reviewed = 0;
    let mut failures = Vec::new();
    let mut repositories = repositories.iter().collect::<Vec<_>>();
    repositories.sort_by_key(|repository| repository["full_name"].as_str().unwrap_or_default());
    'repositories: for repository in repositories {
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
        let github = match GitHub::new(name, token) {
            Ok(github) => github,
            Err(error) => {
                record_sweep_failure(&mut failures, name, &error);
                continue;
            }
        };
        let configuration = match service_configuration(&github) {
            Ok(Some(configuration)) => configuration,
            Ok(None) => continue,
            Err(error) => {
                record_sweep_failure(&mut failures, name, &error);
                continue;
            }
        };
        let Some(checks) = configuration["checks"].as_array().map(|checks| {
            checks
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        }) else {
            continue;
        };
        if checks.is_empty()
            || checks.len() > 32
            || checks.iter().any(|check| {
                check.is_empty() || check.len() > 200 || check.starts_with("Rady dependasolve")
            })
        {
            record_sweep_failure(
                &mut failures,
                name,
                &anyhow!(".github/rady.json has invalid CI evidence names"),
            );
            continue;
        }
        let pulls = match github.pages("pulls?state=open&sort=created&direction=asc", None) {
            Ok(pulls) => pulls,
            Err(error) => {
                record_sweep_failure(&mut failures, name, &error);
                continue;
            }
        };
        for pull in pulls {
            if reviewed >= max_reviews {
                break 'repositories;
            }
            let eligible_author = pull["user"]["login"] == "dependabot[bot]"
                || matches!(
                    pull["author_association"].as_str(),
                    Some("OWNER" | "MEMBER" | "COLLABORATOR")
                );
            if pull["draft"].as_bool() != Some(false)
                || pull["base"]["repo"]["full_name"] != name
                || pull["head"]["repo"]["full_name"] != name
                || !eligible_author
            {
                continue;
            }
            let (Some(number), Some(head)) =
                (pull["number"].as_u64(), pull["head"]["sha"].as_str())
            else {
                record_sweep_failure(
                    &mut failures,
                    name,
                    &anyhow!("GitHub returned an invalid pull request"),
                );
                continue;
            };
            match reviews::review_pr(
                &github,
                number,
                &checks,
                model.as_deref(),
                &slug,
                arguments.harness,
                private,
                "",
                "",
                "",
                head,
                Duration::ZERO,
            ) {
                Ok(outcome) => reviewed += usize::from(outcome.published),
                Err(error) => record_sweep_failure(&mut failures, name, &error),
            }
        }
    }
    Ok(ServiceCycleOutcome { reviewed, failures })
}

fn service_configuration(github: &GitHub) -> Result<Option<Value>> {
    let Some(configuration) = github
        .raw_optional("contents/.github/rady.json")?
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
    else {
        return Ok(None);
    };
    if setup::verified_configuration(github, &configuration)? {
        Ok(Some(configuration))
    } else {
        Ok(None)
    }
}
