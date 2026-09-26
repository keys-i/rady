use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::bail;
use clap::Args;

use crate::Result;
use crate::agent::Harness;
use crate::agent::routing::Intent;
use crate::agent::session::{self, SessionConfig};
use crate::delivery::quality;
use crate::delivery::{self, Config};
use crate::ui::{OutputMode, Theme};

#[derive(Debug, Args)]
pub(crate) struct CodeArgs {
    task: Option<String>,

    #[arg(long)]
    spec: Option<PathBuf>,

    #[arg(long, default_value = ".")]
    directory: PathBuf,

    #[arg(long)]
    model: Option<String>,

    #[arg(long = "model-choice", action = clap::ArgAction::Append)]
    model_choices: Vec<String>,

    #[arg(long)]
    review_model: Option<String>,

    #[arg(long)]
    orchestrator_model: Option<String>,

    #[arg(long, value_enum)]
    orchestrator_harness: Option<Harness>,

    #[arg(long, value_enum, env = "KOELU_HARNESS", default_value = "codex")]
    harness: Harness,

    #[arg(long, default_value_t = 1)]
    agents: usize,

    #[arg(long)]
    max_tokens: Option<u64>,

    #[arg(long)]
    pr: bool,

    #[arg(long)]
    repo: Option<String>,

    #[arg(long)]
    base: Option<String>,

    #[arg(long, hide = true)]
    expected_start: Option<String>,

    #[arg(long = "check", action = clap::ArgAction::Append)]
    checks: Vec<String>,

    #[arg(long = "benchmark", action = clap::ArgAction::Append)]
    benchmarks: Vec<String>,

    #[arg(long, default_value_t = 7)]
    benchmark_runs: usize,

    #[arg(long, default_value_t = 1)]
    benchmark_warmups: usize,

    #[arg(long)]
    benchmark_metric: Option<String>,

    #[arg(long, default_value_t = 10.0)]
    max_benchmark_noise: f64,

    #[arg(long, default_value_t = 5.0)]
    max_regression: f64,

    #[arg(long, default_value_t = 20)]
    max_files: usize,

    #[arg(long, default_value_t = 1000)]
    max_lines: usize,

    #[arg(long, default_value_t = 2)]
    attempts: usize,

    #[arg(long, default_value_t = 1800)]
    timeout: u64,

    /// Enable a named MCP server from .koelu/context.json for write tasks
    #[arg(long = "mcp", action = clap::ArgAction::Append)]
    mcp_servers: Vec<String>,

    /// Use the configured `browser` MCP server for an explicit preview task
    #[arg(long)]
    browser: bool,

    /// Keep remote delivery as one final commit instead of task checkpoints
    #[arg(long)]
    ghost: bool,
}

pub(crate) fn run(arguments: CodeArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let request = quality::load_request(arguments.task.as_deref(), arguments.spec.as_deref())?;
    let mcp_servers = selected_mcp_servers(arguments.mcp_servers, arguments.browser);
    let conversational = arguments.spec.is_none()
        && !arguments.pr
        && arguments.repo.is_none()
        && arguments.base.is_none()
        && arguments.expected_start.is_none()
        && arguments.checks.is_empty()
        && arguments.benchmarks.is_empty()
        && mcp_servers.is_empty()
        && !arguments.ghost
        && request.plan.is_none()
        && request.checks.is_empty()
        && request.benchmarks.is_empty()
        && request.acceptance_checks.is_empty();
    if conversational {
        let config = SessionConfig {
            directory: arguments.directory.clone(),
            harness: arguments.harness,
            model: arguments.model.clone(),
            timeout: Duration::from_secs(arguments.timeout.min(300)),
        };
        if session::classify_request(&request.task, &config) == Intent::ReadOnly {
            let answer = session::ask(&request.task, &config)?;
            return crate::cli::print_session("ask", &answer, theme, output);
        }
    }
    let checks = resolve_checks(
        deduplicate(request.checks.into_iter().chain(arguments.checks)),
        &arguments.directory,
    )?;
    let benchmarks = deduplicate(request.benchmarks.into_iter().chain(arguments.benchmarks));
    if arguments.pr && arguments.repo.is_none() {
        bail!("--pr requires --repo");
    }
    if arguments.repo.is_some() && !arguments.pr {
        bail!("--repo requires --pr");
    }
    delivery::deliver(Config {
        task: request.task,
        directory: arguments.directory,
        repo: arguments.pr.then_some(arguments.repo).flatten(),
        checks,
        harness: arguments.harness,
        agents: arguments.agents,
        model: arguments.model,
        model_choices: arguments.model_choices,
        review_model: arguments.review_model,
        plan: request.plan,
        acceptance_checks: request.acceptance_checks,
        max_tokens: arguments.max_tokens,
        orchestrator_model: arguments.orchestrator_model,
        orchestrator_harness: arguments.orchestrator_harness,
        base: arguments.base,
        attempts: arguments.attempts,
        timeout: Duration::from_secs(arguments.timeout),
        benchmarks,
        benchmark_runs: arguments.benchmark_runs,
        benchmark_warmups: arguments.benchmark_warmups,
        benchmark_metric: arguments.benchmark_metric,
        max_benchmark_noise: arguments.max_benchmark_noise,
        max_regression: arguments.max_regression,
        max_files: arguments.max_files,
        max_lines: arguments.max_lines,
        seed_patch: None,
        resumed_from: None,
        expected_start: arguments.expected_start,
        mcp_servers,
        ghost: arguments.ghost,
        theme,
        output,
    })?;
    Ok(())
}

fn resolve_checks(checks: Vec<String>, directory: &Path) -> Result<Vec<String>> {
    let needs_project_check = checks.is_empty() || checks.iter().any(|check| check == "test");
    if !needs_project_check {
        return Ok(checks);
    }
    let no_checks = checks.is_empty();
    let project_check = infer_project_check(directory)?;
    let fallback = no_checks.then(|| project_check.clone());
    Ok(deduplicate(
        checks
            .into_iter()
            .map(|check| {
                if check == "test" {
                    project_check.clone()
                } else {
                    check
                }
            })
            .chain(fallback),
    ))
}

fn infer_project_check(directory: &Path) -> Result<String> {
    let present = |marker: &str| directory.join(marker).is_file();
    let yarn = present("yarn.lock");
    let pnpm = present("pnpm-lock.yaml");
    let mut candidates = BTreeSet::new();
    if present("Cargo.toml") {
        candidates.insert("cargo test");
    }
    if present("go.mod") {
        candidates.insert("go test ./...");
    }
    if pnpm {
        candidates.insert("pnpm test");
    }
    if yarn {
        candidates.insert("yarn test");
    }
    if present("package-lock.json") || (present("package.json") && !yarn && !pnpm) {
        candidates.insert("npm test");
    }
    if present("pyproject.toml") || present("pytest.ini") || present("tox.ini") {
        candidates.insert("pytest");
    }
    match candidates.len() {
        1 => Ok(candidates
            .pop_first()
            .expect("one candidate exists")
            .to_owned()),
        0 => bail!(
            "could not infer a project test command from {}; add --check \"...\"",
            directory.display()
        ),
        _ => bail!(
            "found multiple project test commands in {}; add --check \"...\"",
            directory.display()
        ),
    }
}

fn deduplicate(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn selected_mcp_servers(mcp_servers: Vec<String>, browser: bool) -> Vec<String> {
    deduplicate(
        mcp_servers
            .into_iter()
            .chain(browser.then(|| "browser".to_owned())),
    )
}

#[cfg(test)]
mod tests {
    use clap::{Args, Command, FromArgMatches};

    use super::*;

    #[test]
    fn deduplication_preserves_first_seen_order() {
        assert_eq!(
            deduplicate(["test".to_owned(), "lint".to_owned(), "test".to_owned()]),
            ["test", "lint"]
        );
    }

    #[test]
    fn code_accepts_the_hidden_pinned_start_revision() {
        let expected_start = "a".repeat(40);
        let command = CodeArgs::augment_args(Command::new("code"));
        let matches = command
            .try_get_matches_from([
                "code".to_owned(),
                "repair dependency conflict".to_owned(),
                "--expected-start".to_owned(),
                expected_start.clone(),
            ])
            .expect("workflow revision must parse");
        let arguments = CodeArgs::from_arg_matches(&matches).expect("arguments must decode");
        assert_eq!(
            arguments.expected_start.as_deref(),
            Some(expected_start.as_str())
        );
    }

    #[test]
    fn browser_flag_selects_the_configured_server_once() {
        let command = CodeArgs::augment_args(Command::new("code"));
        let matches = command
            .try_get_matches_from([
                "code",
                "inspect the preview",
                "--browser",
                "--mcp",
                "browser",
            ])
            .expect("browser flag must parse");
        let arguments = CodeArgs::from_arg_matches(&matches).expect("arguments must decode");
        assert!(arguments.browser);
        assert_eq!(
            selected_mcp_servers(arguments.mcp_servers, arguments.browser),
            ["browser"]
        );
    }

    #[test]
    fn browser_is_never_selected_implicitly() {
        assert_eq!(
            selected_mcp_servers(vec!["docs".to_owned()], false),
            ["docs"]
        );
        assert_eq!(
            selected_mcp_servers(Vec::new(), false),
            Vec::<String>::new()
        );
    }

    #[test]
    fn project_check_inference_avoids_the_posix_test_command() {
        for (marker, expected) in [
            ("Cargo.toml", "cargo test"),
            ("go.mod", "go test ./..."),
            ("package.json", "npm test"),
            ("yarn.lock", "yarn test"),
            ("pnpm-lock.yaml", "pnpm test"),
            ("pyproject.toml", "pytest"),
        ] {
            let directory = tempfile::tempdir().expect("temporary directory");
            std::fs::write(directory.path().join(marker), "").expect("project marker");
            assert_eq!(
                resolve_checks(vec!["test".to_owned()], directory.path())
                    .expect("marker must infer a project check"),
                [expected]
            );
        }

        let directory = tempfile::tempdir().expect("temporary directory");
        let error = resolve_checks(Vec::new(), directory.path())
            .expect_err("unknown projects need an explicit check");
        assert!(error.to_string().contains("add --check \"...\""));
    }
}
