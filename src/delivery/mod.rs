use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Result;
use crate::agent::context::{McpConfiguration, RepositoryContext};
use crate::agent::{self, Harness, Usage};
use crate::github;
use crate::reviews::model::STYLE;
use crate::runs::RunStore;
use crate::ui::{OutputMode, Theme, Ui};
use anyhow::{anyhow, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::benchmark::Measurement;
use self::quality::{AcceptanceCheck, Plan, ReviewReport};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub task: String,
    pub directory: PathBuf,
    pub repo: Option<String>,
    pub checks: Vec<String>,
    pub harness: Harness,
    pub agents: usize,
    pub model: Option<String>,
    pub model_choices: Vec<String>,
    pub review_model: Option<String>,
    pub plan: Option<Plan>,
    pub acceptance_checks: Vec<AcceptanceCheck>,
    pub max_tokens: Option<u64>,
    pub orchestrator_model: Option<String>,
    pub orchestrator_harness: Option<Harness>,
    pub base: Option<String>,
    pub attempts: usize,
    pub timeout: Duration,
    pub benchmarks: Vec<String>,
    pub benchmark_runs: usize,
    pub benchmark_warmups: usize,
    pub benchmark_metric: Option<String>,
    pub max_benchmark_noise: f64,
    pub max_regression: f64,
    pub max_files: usize,
    pub max_lines: usize,
    pub theme: Theme,
    pub output: OutputMode,
    #[serde(default)]
    pub seed_patch: Option<PathBuf>,
    #[serde(default)]
    pub resumed_from: Option<String>,
    #[serde(default)]
    pub expected_start: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub ghost: bool,
}

pub mod benchmark;
mod evidence;
mod git;
mod pull_request;
pub mod quality;
mod retained;
mod run;
mod verification;

#[cfg(test)]
use pull_request::{MAX_PULL_REQUEST_FIELD_BYTES, github_plain_text};

use evidence::{Fingerprint, changed_files, evidence, snapshot};
use git::{
    GitNetworkAuth, checkpoint, git, git_network, git_network_auth, git_network_auth_for_token,
    require_remote_base,
};
use pull_request::{markdown_text, pull_request_body, pull_request_title};
use retained::{Run, retain_candidate, retain_patch};
pub use retained::{apply_run, cancel_run, inspect_run, list_runs, resume_run};
use run::deliver_with_auth;
use verification::{acceptance, run_check, unchanged, verification_files};

struct RepositoryWorkspace<'a> {
    directory: &'a Path,
    workspace: &'a Path,
    branch: &'a str,
    base: Option<&'a str>,
    origin: Option<&'a str>,
    network_auth: Option<&'a GitNetworkAuth>,
    hosted_auth: Option<&'a DeliveryAuth>,
}

/// An installation token bound to one repository for a hosted delivery run
pub(crate) struct DeliveryAuth {
    repository: String,
    token: String,
    publication: Option<HostedWriteAuthorization>,
}

struct HostedWriteAuthorization {
    issue: u64,
    approved: crate::mentions::ApprovedWrite,
    claim_comment: u64,
}

impl DeliveryAuth {
    fn installation(repository: &str, token: impl Into<String>) -> Result<Self> {
        github::validate_repository(repository)?;
        let token = token.into();
        if token.is_empty()
            || token.len() > git::MAX_PUSH_TOKEN_BYTES
            || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            bail!("a bounded GitHub App installation token is required");
        }
        Ok(Self {
            repository: repository.to_owned(),
            token,
            publication: None,
        })
    }

    pub(crate) fn approved_installation(
        repository: &str,
        token: impl Into<String>,
        issue: u64,
        approved: &crate::mentions::ApprovedWrite,
        claim_comment: u64,
    ) -> Result<Self> {
        if issue == 0 || claim_comment == 0 {
            bail!("a bound hosted write approval is required");
        }
        let mut auth = Self::installation(repository, token)?;
        auth.publication = Some(HostedWriteAuthorization {
            issue,
            approved: approved.clone(),
            claim_comment,
        });
        Ok(auth)
    }

    fn repository_info(&self) -> Result<Value> {
        github::api_authenticated(
            &format!("repos/{}", self.repository),
            None,
            "GET",
            false,
            &self.token,
        )?
        .ok_or_else(|| anyhow!("GitHub returned no repository"))
    }

    fn validate(&self, config: &Config) -> Result<()> {
        if config.repo.as_deref() != Some(self.repository.as_str()) {
            bail!("hosted delivery authentication must match the selected repository");
        }
        if self.publication.is_none() {
            bail!("hosted delivery requires a bound approved write");
        }
        Ok(())
    }

    fn network_auth(&self, remote: &str) -> Result<GitNetworkAuth> {
        git_network_auth_for_token(remote, &self.token)
    }

    fn token(&self) -> &str {
        &self.token
    }

    fn revalidate_publication(&self) -> Result<()> {
        let Some(expected) = self.publication.as_ref() else {
            bail!("hosted delivery requires a bound approved write");
        };
        let github = github::GitHub::new(&self.repository, &self.token)?;
        let Some(actual) = crate::mentions::approved_write_with_claim(
            &github,
            expected.issue,
            expected.approved.approval_comment,
            expected.claim_comment,
        )?
        else {
            bail!("the hosted write approval changed before publication");
        };
        if actual != expected.approved {
            bail!("the hosted write approval no longer matches this delivery");
        }
        Ok(())
    }
}

pub fn deliver(config: Config) -> Result<Value> {
    deliver_with_auth(config, None)
}

/// Deliver from a hosted service using a repository-scoped installation token
pub(crate) fn deliver_hosted(config: Config, auth: DeliveryAuth) -> Result<Value> {
    deliver_with_auth(config, Some(auth))
}

fn require_safe_hosted_publication_paths(paths: &[String]) -> Result<()> {
    if let Some(path) = paths.iter().find(|path| {
        path.starts_with(".github/workflows/")
            || path.starts_with(".github/actions/")
            || *path == ".github/pekin.json"
            || matches!(
                path.as_str(),
                ".env" | "AGENTS.md" | "DESIGN.md" | "GEMINI.md"
            )
            || path.starts_with(".pekin/")
            || path.starts_with(".gemini/")
    }) {
        bail!("hosted delivery cannot publish protected path: {path}");
    }
    Ok(())
}

fn validate_config(config: &Config) -> Result<()> {
    if config.task.trim().is_empty() || config.task.chars().count() > 32_000 {
        bail!("provide a task between 1 and 32000 characters");
    }
    if config.checks.is_empty()
        || config.checks.len() > 20
        || config.benchmarks.len() > 5
        || config
            .checks
            .iter()
            .chain(&config.benchmarks)
            .map(String::len)
            .sum::<usize>()
            > 4_000
    {
        bail!("provide 1-20 checks and at most 5 benchmarks within 4000 characters");
    }
    if !(1..=8).contains(&config.agents)
        || !(1..=3).contains(&config.attempts)
        || config.timeout.is_zero()
        || config.timeout > Duration::from_secs(86_400)
        || config.max_files == 0
        || config.max_files > 1_000
        || config.max_lines == 0
        || config.max_lines > 100_000
    {
        bail!("use 1-8 agents, 1-3 attempts, a timeout up to 24 hours and bounded scope limits");
    }
    if config.max_tokens.is_some() && config.agents != 1 {
        bail!("a token budget requires one agent; subagent usage is not verified");
    }
    if config.repo.is_some() && config.acceptance_checks.is_empty() {
        bail!("automatic PR delivery requires fixed acceptance checks for every criterion");
    }
    if config.model_choices.len() > 8
        || config
            .model_choices
            .iter()
            .any(|value| value.trim().is_empty() || value.len() > 200)
        || config.model_choices.iter().collect::<BTreeSet<_>>().len() != config.model_choices.len()
    {
        bail!("provide up to eight distinct model choices");
    }
    if config.mcp_servers.len() > 8
        || config
            .mcp_servers
            .iter()
            .any(|name| name.is_empty() || name.len() > 64)
        || config.mcp_servers.iter().collect::<BTreeSet<_>>().len() != config.mcp_servers.len()
    {
        bail!("select up to eight distinct configured MCP servers");
    }
    if config.ghost && config.repo.is_none() {
        bail!("--ghost applies only to remote pull-request delivery");
    }
    if config.expected_start.as_ref().is_some_and(|value| {
        !matches!(value.len(), 40 | 64)
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        bail!("expected source revision must be a full hexadecimal object ID");
    }
    Ok(())
}

fn repository_from_remote(remote: &str) -> Option<String> {
    let trimmed = remote.strip_suffix(".git").unwrap_or(remote);
    for prefix in [
        "https://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ] {
        if let Some(value) = trimmed.strip_prefix(prefix) {
            return Some(value.to_owned());
        }
    }
    None
}

fn changed_between(
    before: &BTreeMap<String, Fingerprint>,
    after: &BTreeMap<String, Fingerprint>,
) -> Vec<String> {
    before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|name| before.get(*name) != after.get(*name))
        .cloned()
        .collect()
}

fn parse_commands(values: &[String]) -> Result<Vec<Vec<String>>> {
    values
        .iter()
        .map(|value| agent::split_command(value))
        .collect()
}

fn join_command(command: &[String]) -> String {
    command
        .iter()
        .map(|value| {
            if value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-._/:".contains(character))
            {
                value.clone()
            } else {
                format!("'{}'", value.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn tail(value: &str, bytes: usize) -> &str {
    if value.len() <= bytes {
        return value;
    }
    let mut start = value.len() - bytes;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut value = vec![0_u8; bytes];
    getrandom::fill(&mut value)
        .map_err(|error| anyhow!("secure randomness is unavailable: {error}"))?;
    Ok(hex(&value))
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
mod tests;
