use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::bail;
use clap::Args;
use serde_json::Value;

use crate::Result;
use crate::agent::Harness;
use crate::github;

mod mentions;
mod reviews;
mod tokens;

use tokens::ServiceTokenProvider;

#[derive(Debug, Args)]
pub(crate) struct ServeArgs {
    /// Restrict the service to one installed account
    #[arg(long)]
    owner: Option<String>,

    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,

    #[arg(long, default_value_t = 30)]
    interval: u64,

    #[arg(long, default_value_t = 4)]
    max_reviews: usize,

    /// GitHub App client ID used to mint installation tokens automatically
    #[arg(long, env = "RADY_APP_CLIENT_ID")]
    app_client_id: Option<String>,

    #[arg(long, env = "RADY_APP_ID", hide = true)]
    app_id: Option<String>,

    /// Path to the GitHub App RSA private key
    #[arg(long, env = "RADY_APP_PRIVATE_KEY_FILE", value_name = "PEM")]
    app_private_key_file: Option<PathBuf>,

    #[arg(long, env = "RADY_APP_TOKEN_COMMAND", hide = true)]
    token_command: Option<String>,

    #[arg(long)]
    once: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SweepArgs {
    #[arg(long)]
    owner: Option<String>,
    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,
}

pub(super) const MAX_SWEEP_FAILURES: usize = 8;

pub(crate) fn sweep(arguments: SweepArgs) -> Result<()> {
    mentions::sweep(arguments)
}

pub(crate) fn serve(arguments: ServeArgs) -> Result<()> {
    validate_owner_filter(arguments.owner.as_deref())?;
    if !(5..=3_600).contains(&arguments.interval) || !(1..=16).contains(&arguments.max_reviews) {
        bail!("use a service interval from 5 to 3600 seconds and 1-16 reviews per cycle");
    }
    let mut tokens = ServiceTokenProvider::new(&arguments)?;
    let mut consecutive_failures = 0_u8;
    let mut mention_cursors = BTreeMap::new();
    loop {
        let cycle = service_cycle(&arguments, &mut tokens, &mut mention_cursors);
        if let Ok(reviewed) = &cycle {
            consecutive_failures = 0;
            if *reviewed == 0 {
                eprintln!("No pull requests needed a review this pass");
            } else {
                let noun = if *reviewed == 1 {
                    "pull request"
                } else {
                    "pull requests"
                };
                eprintln!("Reviewed {reviewed} {noun} this pass");
            }
        } else if !arguments.once {
            consecutive_failures = consecutive_failures.saturating_add(1);
            eprintln!("This pass couldn't finish: {}", cycle.as_ref().unwrap_err());
        }
        if arguments.once {
            return cycle.map(|_| ());
        }
        if consecutive_failures >= 3 {
            bail!("service stopped after three failed passes");
        }
        thread::sleep(Duration::from_secs(arguments.interval));
    }
}

fn service_cycle(
    arguments: &ServeArgs,
    tokens: &mut ServiceTokenProvider,
    mention_cursors: &mut BTreeMap<String, u64>,
) -> Result<usize> {
    let mut reviewed = 0;
    let mut failures = Vec::new();
    for (index, token) in tokens.tokens()?.iter().enumerate() {
        let remaining = arguments.max_reviews.saturating_sub(reviewed);
        let outcome = service_installation_cycle(arguments, token, mention_cursors, remaining);
        merge_service_outcome(&mut reviewed, &mut failures, index + 1, outcome);
    }
    if failures.is_empty() {
        Ok(reviewed)
    } else {
        let noun = if failures.len() == 1 {
            "problem"
        } else {
            "problems"
        };
        bail!(
            "this pass hit {} installation {noun}: {}",
            failures.len(),
            failures.join("; ")
        )
    }
}

#[derive(Default)]
pub(super) struct ServiceCycleOutcome {
    pub(super) reviewed: usize,
    pub(super) failures: Vec<String>,
}

fn merge_service_outcome(
    reviewed: &mut usize,
    failures: &mut Vec<String>,
    installation: usize,
    outcome: ServiceCycleOutcome,
) {
    *reviewed = reviewed.saturating_add(outcome.reviewed);
    for failure in outcome.failures {
        if failures.len() < MAX_SWEEP_FAILURES {
            failures.push(format!("installation {installation}: {failure}"));
        }
    }
}

fn service_installation_cycle(
    arguments: &ServeArgs,
    token: &str,
    mention_cursors: &mut BTreeMap<String, u64>,
    max_reviews: usize,
) -> ServiceCycleOutcome {
    let mentions = mentions::sweep_with_token(
        &SweepArgs {
            owner: arguments.owner.clone(),
            harness: arguments.harness,
        },
        token,
        mention_cursors,
    );
    let reviews = if max_reviews == 0 {
        Ok(ServiceCycleOutcome::default())
    } else {
        reviews::service_reviews(arguments, token, max_reviews)
    };
    let mut outcome = reviews.unwrap_or_else(|error| ServiceCycleOutcome {
        reviewed: 0,
        failures: vec![format!("pull-request sweep: {error}")],
    });
    if let Err(error) = mentions {
        outcome.failures.push(format!("mention sweep: {error}"));
    }
    outcome
}

pub(super) fn record_sweep_failure(failures: &mut Vec<String>, repo: &str, error: &anyhow::Error) {
    if failures.len() < MAX_SWEEP_FAILURES {
        failures.push(format!("{repo}: {error}"));
    }
}

pub(super) fn validate_owner_filter(owner: Option<&str>) -> Result<()> {
    if let Some(owner) = owner {
        github::validate_repository(&format!("{owner}/rady"))?;
    }
    Ok(())
}

pub(super) fn owner_matches(repository: &Value, owner: Option<&str>) -> bool {
    owner.is_none_or(|owner| {
        repository["owner"]["login"]
            .as_str()
            .is_some_and(|login| login.eq_ignore_ascii_case(owner))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_boundaries_are_narrow_and_cross_owner() {
        let repository = serde_json::json!({"owner": {"login": "Keys-I"}});
        assert!(owner_matches(&repository, None));
        assert!(owner_matches(&repository, Some("keys-i")));
        assert!(!owner_matches(&repository, Some("other")));
        assert!(validate_owner_filter(Some("keys-i")).is_ok());
        assert!(validate_owner_filter(Some("bad owner")).is_err());

        let mut reviewed = 0;
        let mut failures = Vec::new();
        merge_service_outcome(
            &mut reviewed,
            &mut failures,
            1,
            ServiceCycleOutcome {
                reviewed: 1,
                failures: vec!["temporary failure".to_owned()],
            },
        );
        merge_service_outcome(
            &mut reviewed,
            &mut failures,
            2,
            ServiceCycleOutcome {
                reviewed: 2,
                failures: Vec::new(),
            },
        );
        assert_eq!(reviewed, 3);
        assert_eq!(failures, ["installation 1: temporary failure"]);
    }
}
