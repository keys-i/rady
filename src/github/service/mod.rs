use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::bail;
use clap::Args;
use serde_json::{Value, json};

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

    #[arg(long, value_enum, env = "PEKIN_HARNESS", default_value = "codex")]
    harness: Harness,

    #[arg(long, default_value_t = 30)]
    interval: u64,

    #[arg(long, default_value_t = 4, hide = true)]
    max_reviews: usize,

    /// GitHub App client ID used to mint installation tokens automatically
    #[arg(long, env = "PEKIN_APP_CLIENT_ID")]
    app_client_id: Option<String>,

    /// Path to the GitHub App RSA private key
    #[arg(long, env = "PEKIN_APP_PRIVATE_KEY_FILE", value_name = "PEM")]
    app_private_key_file: Option<PathBuf>,

    #[arg(long)]
    once: bool,
}

pub(super) const MAX_SWEEP_FAILURES: usize = 8;

pub(crate) fn serve(arguments: ServeArgs) -> Result<()> {
    validate_owner_filter(arguments.owner.as_deref())?;
    if !(5..=3_600).contains(&arguments.interval) {
        bail!("use a service interval from 5 to 3600 seconds");
    }
    let mut tokens =
        ServiceTokenProvider::new(&arguments, github::InstallationTokenScope::Mentions)?;
    let mut consecutive_failures = 0_u8;
    let mut mention_cursors = BTreeMap::new();
    loop {
        let cycle = service_cycle(&arguments, &mut tokens, &mut mention_cursors);
        if let Ok(()) = &cycle {
            consecutive_failures = 0;
            eprintln!("Mention pass complete");
        } else if !arguments.once {
            consecutive_failures = consecutive_failures.saturating_add(1);
            eprintln!("This pass couldn't finish: {}", cycle.as_ref().unwrap_err());
        }
        if arguments.once {
            return cycle;
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
) -> Result<()> {
    let mut failures = Vec::new();
    let installation_tokens = tokens.tokens()?.to_owned();
    for (index, token) in installation_tokens.iter().enumerate() {
        let outcome = service_installation_cycle(arguments, token, tokens, mention_cursors);
        merge_service_outcome(&mut failures, index + 1, outcome);
    }
    if failures.is_empty() {
        Ok(())
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
    pub(super) failures: Vec<String>,
}

fn merge_service_outcome(
    failures: &mut Vec<String>,
    installation: usize,
    outcome: ServiceCycleOutcome,
) {
    for failure in outcome.failures {
        if failures.len() < MAX_SWEEP_FAILURES {
            failures.push(format!("installation {installation}: {failure}"));
        }
    }
}

fn service_installation_cycle(
    arguments: &ServeArgs,
    token: &str,
    tokens: &ServiceTokenProvider,
    mention_cursors: &mut BTreeMap<String, u64>,
) -> ServiceCycleOutcome {
    let mentions = mentions::sweep_with_token(arguments, token, tokens, mention_cursors);
    let mut outcome = ServiceCycleOutcome::default();
    if let Err(error) = mentions {
        outcome.failures.push(format!("mention sweep: {error}"));
    }
    outcome
}

pub(crate) fn targets(arguments: ServeArgs) -> Result<()> {
    validate_owner_filter(arguments.owner.as_deref())?;
    if !(1..=100).contains(&arguments.max_reviews) {
        bail!("use a target limit from 1 to 100");
    }
    let seed = env::var("PEKIN_TARGET_SEED")
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| anyhow::anyhow!("PEKIN_TARGET_SEED must be a non-negative integer"))?
        .unwrap_or_default();
    let mut tokens =
        ServiceTokenProvider::new(&arguments, github::InstallationTokenScope::Targets)?;
    let mut targets = TargetWindow::new(arguments.max_reviews, seed);
    let mut failures = Vec::new();
    for (index, token) in tokens.tokens()?.iter().enumerate() {
        match reviews::central_targets(&arguments, token, &mut |target| targets.consider(target)) {
            Ok(()) => {}
            Err(error) => {
                if failures.len() < MAX_SWEEP_FAILURES {
                    failures.push(format!("installation {}: {error}", index + 1));
                }
            }
        }
    }
    if !failures.is_empty() {
        bail!("target discovery hit {}", failures.join("; "));
    }
    println!(
        "{}",
        serde_json::to_string(&json!({"include": targets.into_targets()}))?
    );
    Ok(())
}

struct TargetWindow {
    maximum: usize,
    seed: usize,
    targets: Vec<reviews::CentralTarget>,
}

impl TargetWindow {
    fn new(maximum: usize, seed: usize) -> Self {
        Self {
            maximum,
            seed,
            targets: Vec::with_capacity(maximum),
        }
    }

    fn consider(&mut self, target: reviews::CentralTarget) {
        if self.targets.len() < self.maximum {
            self.targets.push(target);
            return;
        }
        let Some((worst, _)) = self
            .targets
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| self.compare(left, right))
        else {
            return;
        };
        if self.compare(&target, &self.targets[worst]) == Ordering::Less {
            self.targets[worst] = target;
        }
    }

    fn into_targets(mut self) -> Vec<reviews::CentralTarget> {
        let seed = self.seed;
        self.targets.sort_by(|left, right| {
            target_rank(seed, left)
                .cmp(&target_rank(seed, right))
                .then_with(|| (&left.repo, left.number).cmp(&(&right.repo, right.number)))
        });
        self.targets
    }

    fn compare(&self, left: &reviews::CentralTarget, right: &reviews::CentralTarget) -> Ordering {
        target_rank(self.seed, left)
            .cmp(&target_rank(self.seed, right))
            .then_with(|| (&left.repo, left.number).cmp(&(&right.repo, right.number)))
    }
}

fn target_rank(seed: usize, target: &reviews::CentralTarget) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in format!("{seed}:{}#{}", target.repo, target.number).bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(super) fn record_sweep_failure(failures: &mut Vec<String>, repo: &str, error: &anyhow::Error) {
    if failures.len() < MAX_SWEEP_FAILURES {
        failures.push(format!("{repo}: {error}"));
    }
}

pub(super) fn validate_owner_filter(owner: Option<&str>) -> Result<()> {
    if let Some(owner) = owner {
        github::validate_repository(&format!("{owner}/pekin"))?;
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

        let mut failures = Vec::new();
        merge_service_outcome(
            &mut failures,
            1,
            ServiceCycleOutcome {
                failures: vec!["temporary failure".to_owned()],
            },
        );
        merge_service_outcome(
            &mut failures,
            2,
            ServiceCycleOutcome {
                failures: Vec::new(),
            },
        );
        assert_eq!(failures, ["installation 1: temporary failure"]);

        let target = reviews::CentralTarget {
            repo: "keys-i/rady".to_owned(),
            owner: "keys-i".to_owned(),
            name: "rady".to_owned(),
            private: false,
            number: 7,
            checks: vec!["test".to_owned()],
            solver_ref: "keys-i/rady@0123456789abcdef0123456789abcdef01234567".to_owned(),
        };
        assert_eq!(
            serde_json::json!({"include": [target]}),
            serde_json::json!({
                "include": [{
                    "repo": "keys-i/rady", "owner": "keys-i", "name": "rady",
                    "private": false, "number": 7, "checks": ["test"],
                    "solver_ref": "keys-i/rady@0123456789abcdef0123456789abcdef01234567"
                }]
            })
        );

        let targets = (1..=150)
            .map(|number| reviews::CentralTarget {
                repo: "keys-i/rady".to_owned(),
                owner: "keys-i".to_owned(),
                name: "rady".to_owned(),
                private: false,
                number,
                checks: vec!["test".to_owned()],
                solver_ref: "keys-i/rady@0123456789abcdef0123456789abcdef01234567".to_owned(),
            })
            .collect::<Vec<_>>();
        let choose = |seed| {
            let mut window = TargetWindow::new(4, seed);
            for target in targets.clone() {
                window.consider(target);
            }
            window
                .into_targets()
                .iter()
                .map(|target| target.number)
                .collect::<Vec<_>>()
        };
        let first = choose(0);
        let next = choose(4);
        assert_eq!(first.len(), 4);
        assert_eq!(next.len(), 4);
        assert_ne!(first, next);
    }
}
