use anyhow::anyhow;
use serde::Serialize;
use serde_json::Value;

use crate::Result;
use crate::github::{self, GitHub};
use crate::setup;

use super::{ServeArgs, owner_matches, record_sweep_failure};

#[derive(Clone, Debug, Serialize)]
pub(super) struct CentralTarget {
    pub(super) repo: String,
    pub(super) owner: String,
    pub(super) name: String,
    pub(super) private: bool,
    pub(super) number: u64,
    pub(super) checks: Vec<String>,
    pub(super) solver_ref: String,
}

pub(super) fn central_targets<F>(arguments: &ServeArgs, token: &str, select: &mut F) -> Result<()>
where
    F: FnMut(CentralTarget),
{
    let repositories =
        github::authenticated_pages("installation/repositories", "repositories", token)?;
    let mut failures = Vec::new();
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
        let solver_ref = match solver_ref(&configuration) {
            Some(Ok(source)) => source.joined(),
            Some(Err(error)) => {
                record_sweep_failure(
                    &mut failures,
                    name,
                    &anyhow!(".github/rady.json has an invalid trusted solver source: {error}"),
                );
                continue;
            }
            None => {
                record_sweep_failure(
                    &mut failures,
                    name,
                    &anyhow!(".github/rady.json has no trusted solver source"),
                );
                continue;
            }
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
            let dependabot = pull["user"]["login"] == "dependabot[bot]";
            let eligible_author = dependabot
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
            let (Some(number), Some(owner), Some(repository_name)) = (
                pull["number"].as_u64(),
                repository["owner"]["login"].as_str(),
                repository["name"].as_str(),
            ) else {
                record_sweep_failure(
                    &mut failures,
                    name,
                    &anyhow!("GitHub returned an invalid pull request"),
                );
                continue;
            };
            let Some(private) = private else {
                record_sweep_failure(
                    &mut failures,
                    name,
                    &anyhow!("GitHub returned an invalid repository visibility"),
                );
                continue;
            };
            select(CentralTarget {
                repo: name.to_owned(),
                owner: owner.to_owned(),
                name: repository_name.to_owned(),
                private,
                number,
                checks: checks.clone(),
                solver_ref: solver_ref.clone(),
            });
        }
    }
    for failure in failures {
        eprintln!("Target skipped: {failure}");
    }
    Ok(())
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

fn solver_ref(configuration: &Value) -> Option<Result<setup::SourceRef>> {
    configuration["source"]
        .as_str()
        .map(setup::SourceRef::parse)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::solver_ref;

    #[test]
    fn solver_source_is_an_explicit_trusted_commit() {
        for (source, valid) in [
            ("keys-i/rady@0123456789abcdef0123456789abcdef01234567", true),
            ("keys-i/rady@main", false),
            ("other/rady@0123456789abcdef0123456789abcdef01234567", false),
        ] {
            let configuration = json!({"source": source});
            assert_eq!(
                solver_ref(&configuration).is_some_and(|value| value.is_ok()),
                valid
            );
        }
        assert!(solver_ref(&json!({})).is_none());
    }
}
