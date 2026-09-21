use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

use crate::Result;
use crate::agent::{self, Harness};
use crate::apps::Identity;
use crate::delivery::{self, Config};
use crate::github::GitHub;
use crate::quality;
use crate::reviews;
use crate::setup::{self, SourceRef};
use crate::ui::{OutputMode, Theme, Ui, json_success_document, print_markdown};

#[derive(Debug, Parser)]
#[command(
    name = "rady",
    version,
    about = "Human-first, evidence-gated coding and dependency review"
)]
struct Cli {
    #[arg(long, value_enum, global = true, default_value = "auto")]
    theme: Theme,

    #[arg(long, value_enum, global = true, default_value = "human")]
    output: OutputMode,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Turn a request into a checked local change or pull request
    Code(Box<CodeArgs>),

    /// Configure Rady's Dependabot review and merge gates
    #[command(alias = "dependasolver")]
    Dependasolve(DependSolveArgs),

    /// List retained coding runs
    Runs,

    /// Show the evidence retained for a coding run
    Inspect(RunArgs),

    /// Stop a running coding run
    Cancel(RunArgs),

    /// Restart a stopped run from its retained patch
    Resume(RunArgs),

    /// Apply a verified retained patch to a clean directory
    Apply(ApplyArgs),

    /// Check native harness logins and delivery tools
    Doctor(DoctorArgs),

    /// Pass arguments to a native agent harness unchanged
    Agent(AgentArgs),

    #[command(hide = true)]
    Resolve(ResolveArgs),

    #[command(hide = true)]
    Review(ReviewArgs),
}

#[derive(Debug, Args)]
struct CodeArgs {
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

    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
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
}

#[derive(Debug, Args)]
struct DependSolveArgs {
    #[arg(long)]
    repo: String,

    /// Optional trusted solver source; defaults to the latest keys-i/rady commit
    #[arg(long)]
    solver_ref: Option<String>,

    #[arg(long = "checks", required = true, num_args = 1..)]
    checks: Vec<String>,

    #[arg(long, default_value = ".")]
    directory: PathBuf,

    #[arg(long = "app", value_enum, default_value = "dependasolver")]
    identity: Identity,

    #[arg(long)]
    new_app: bool,

    /// Refuse to replace an existing generated workflow
    #[arg(long)]
    no_overwrite: bool,

    #[arg(long)]
    apply: bool,
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Retained run identifier
    run: String,
}

#[derive(Debug, Args)]
struct ApplyArgs {
    /// Retained run identifier
    run: String,

    #[arg(long, default_value = ".")]
    directory: PathBuf,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,
}

#[derive(Debug, Args)]
struct AgentArgs {
    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    arguments: Vec<OsString>,
}

#[derive(Debug, Args)]
struct ResolveArgs {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    pr: u64,
}

#[derive(Debug, Args)]
struct ReviewArgs {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    pr: u64,
    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,
}

pub fn run() -> Result<()> {
    run_from(env::args_os())
}

const MAX_ERROR_CHARACTERS: usize = 8_000;

#[derive(Debug)]
struct CliFailure {
    output: OutputMode,
    kind: &'static str,
    message: String,
    exit_code: u8,
}

impl CliFailure {
    fn new(output: OutputMode, error: &anyhow::Error) -> Self {
        Self {
            output,
            kind: "runtime",
            message: bounded_message(format_args!("{error:#}")),
            exit_code: 1,
        }
    }

    fn usage(output: OutputMode, error: &clap::Error) -> Self {
        Self {
            output,
            kind: "usage",
            message: bounded_message(format_args!("{error}")),
            exit_code: u8::try_from(error.exit_code()).unwrap_or(2),
        }
    }
}

impl std::fmt::Display for CliFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliFailure {}

struct BoundedMessage {
    value: String,
    characters: usize,
    truncated: bool,
}

impl fmt::Write for BoundedMessage {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let remaining = MAX_ERROR_CHARACTERS.saturating_sub(self.characters);
        if remaining == 0 {
            self.truncated = true;
            return Err(fmt::Error);
        }
        if let Some((boundary, _)) = value.char_indices().nth(remaining) {
            self.value.push_str(&value[..boundary]);
            self.characters = MAX_ERROR_CHARACTERS;
            self.truncated = true;
            return Err(fmt::Error);
        }
        self.value.push_str(value);
        self.characters += value.chars().count();
        Ok(())
    }
}

fn bounded_message(arguments: fmt::Arguments<'_>) -> String {
    let mut message = BoundedMessage {
        value: String::with_capacity(MAX_ERROR_CHARACTERS),
        characters: 0,
        truncated: false,
    };
    let _ = message.write_fmt(arguments);
    if message.truncated {
        message.value.push_str("\n[Error truncated]");
    }
    message.value
}

fn json_error_document(kind: &str, message: &str) -> String {
    serde_json::to_string(&json!({
        "schema": 1,
        "status": "error",
        "error": {"kind": kind, "message": message},
    }))
    .unwrap_or_else(|_| {
        "{\"schema\":1,\"status\":\"error\",\"error\":{\"kind\":\"serialization\",\"message\":\"Could not encode the error\"}}".to_owned()
    })
}

pub fn error_exit(error: anyhow::Error) -> ExitCode {
    if let Some(error) = error.downcast_ref::<clap::Error>() {
        let _ = error.print();
        return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1));
    }
    if let Some(failure) = error.downcast_ref::<CliFailure>() {
        if failure.output == OutputMode::Json {
            eprintln!("{}", json_error_document(failure.kind, &failure.message));
        } else {
            eprintln!("Rady stopped: {}", failure.message);
        }
        return ExitCode::from(failure.exit_code);
    } else {
        eprintln!("Rady stopped: {error:#}");
    }
    ExitCode::FAILURE
}

pub fn run_from<I, T>(arguments: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let arguments = arguments.into_iter().map(Into::into).collect::<Vec<_>>();
    let requested_output = requested_output(&arguments);
    let cli = Cli::try_parse_from(arguments).map_err(|error| {
        if requested_output == OutputMode::Json && error.exit_code() != 0 {
            anyhow::Error::new(CliFailure::usage(requested_output, &error))
        } else {
            error.into()
        }
    })?;
    let output = cli.output;
    let result = match cli.command {
        Commands::Code(arguments) => code(*arguments, cli.theme, cli.output),
        Commands::Dependasolve(arguments) => dependasolve(arguments, cli.theme, cli.output),
        Commands::Runs => delivery::list_runs(cli.theme, cli.output),
        Commands::Inspect(arguments) => {
            delivery::inspect_run(&arguments.run, cli.theme, cli.output)
        }
        Commands::Cancel(arguments) => delivery::cancel_run(&arguments.run, cli.theme, cli.output),
        Commands::Resume(arguments) => delivery::resume_run(&arguments.run, cli.theme, cli.output),
        Commands::Apply(arguments) => {
            delivery::apply_run(&arguments.run, &arguments.directory, cli.theme, cli.output)
        }
        Commands::Doctor(arguments) => doctor(arguments, cli.theme, cli.output),
        Commands::Agent(arguments) => native_agent(arguments),
        Commands::Resolve(arguments) => resolve(arguments),
        Commands::Review(arguments) => review(arguments),
    };
    result.map_err(|error| CliFailure::new(output, &error).into())
}

fn requested_output(arguments: &[OsString]) -> OutputMode {
    let mut output = OutputMode::Human;
    let mut arguments = arguments.iter().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            break;
        }
        if argument == "--output" {
            if arguments.next().is_some_and(|value| value == "json") {
                output = OutputMode::Json;
            }
        } else if argument == "--output=json" {
            output = OutputMode::Json;
        }
    }
    output
}

fn code(arguments: CodeArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let request = quality::load_request(arguments.task.as_deref(), arguments.spec.as_deref())?;
    let checks = deduplicate(request.checks.into_iter().chain(arguments.checks));
    let benchmarks = deduplicate(request.benchmarks.into_iter().chain(arguments.benchmarks));
    if checks.is_empty() {
        bail!("code requires a --check or JSON spec checks");
    }
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
        expected_start: None,
        theme,
        output,
    })?;
    Ok(())
}

fn dependasolve(arguments: DependSolveArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let mut ui = Ui::new(theme, output, 3);
    ui.title(
        "Rady dependasolve",
        "Dependency updates, grounded in evidence",
    );
    ui.stage("Resolving the trusted solver source");
    let source = SourceRef::resolve(arguments.solver_ref.as_deref())?;
    ui.stage("Validating repository setup");
    let preview = setup::run(
        &arguments.repo,
        &source,
        &arguments.checks,
        &arguments.directory,
        arguments.identity,
        arguments.new_app,
        !arguments.no_overwrite,
        arguments.apply,
    )?;
    ui.stage(if arguments.apply {
        "Repository configured"
    } else {
        "Preview ready"
    });
    if output == OutputMode::Json {
        println!("{}", json_success_document("dependasolve", &preview)?);
    } else {
        let files = preview["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|path| format!("- `{path}`"))
            .collect::<Vec<_>>()
            .join("\n");
        let markdown = format!(
            "## {}\n\n**Repository:** `{}`\n\n**Source:** `{}`\n\n### Files\n\n{}\n\n{}",
            if arguments.apply {
                "Setup complete"
            } else {
                "Setup preview"
            },
            arguments.repo,
            source.joined(),
            files,
            if arguments.apply {
                "Repository settings and App credentials are configured."
            } else {
                "Run again with `--apply` after reviewing this preview."
            }
        );
        print_markdown(&markdown, theme)?;
    }
    Ok(())
}

fn doctor(arguments: DoctorArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let mut ui = Ui::new(theme, output, 3);
    ui.title("Rady doctor", "A quick readiness check");
    let mut rows = Vec::new();
    let mut ready = true;
    for name in ["git", "gh"] {
        let installed = agent::which(name).is_some();
        ready &= installed;
        ui.stage(&format!(
            "{name}: {}",
            if installed { "installed" } else { "missing" }
        ));
        rows.push(json!({"requirement": name, "ready": installed}));
    }
    let harness = match agent::executable(arguments.harness) {
        Ok(_) => true,
        Err(error) => {
            ui.warning(&error.to_string());
            false
        }
    };
    ready &= harness;
    ui.stage(&format!(
        "{}: {}",
        arguments.harness.as_str(),
        if harness { "ready" } else { "needs attention" }
    ));
    rows.push(json!({"requirement": arguments.harness.as_str(), "ready": harness}));
    if output == OutputMode::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"ready": ready, "requirements": rows}))?
        );
    } else if ready {
        ui.success("Local coding is ready");
        ui.note("PR delivery also needs repository push access and fixed acceptance checks");
    }
    if ready {
        Ok(())
    } else {
        bail!("one or more readiness checks failed")
    }
}

fn native_agent(arguments: AgentArgs) -> Result<()> {
    let (program, prefix) = if arguments.harness == Harness::Command {
        let configured = agent::split_command(&env::var("RADY_AGENT_COMMAND").unwrap_or_default())?;
        let (program, prefix) = configured
            .split_first()
            .ok_or_else(|| anyhow!("set RADY_AGENT_COMMAND"))?;
        (
            program.clone(),
            prefix.iter().map(OsString::from).collect::<Vec<_>>(),
        )
    } else {
        (arguments.harness.as_str().to_owned(), Vec::new())
    };
    let binary = agent::which(&program).ok_or_else(|| anyhow!("install the selected harness"))?;
    let mut command = Command::new(binary);
    command.args(prefix).args(arguments.arguments);
    for name in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "ANTHROPIC_API_KEY",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        command.env_remove(name);
    }
    let status = command
        .status()
        .context("could not run native harness command")?;
    if status.success() {
        Ok(())
    } else {
        bail!("native harness exited with {}", status.code().unwrap_or(1))
    }
}

fn resolve(arguments: ResolveArgs) -> Result<()> {
    if arguments.pr == 0 {
        bail!("PR number must be positive");
    }
    let github = GitHub::new(&arguments.repo, &env::var("GH_TOKEN").unwrap_or_default())?;
    let (pull, dependency) = reviews::resolve(&github, arguments.pr)?;
    action_output(&[
        ("dependency", dependency.to_string()),
        (
            "head",
            pull["head"]["sha"].as_str().unwrap_or_default().to_owned(),
        ),
    ])
}

fn review(arguments: ReviewArgs) -> Result<()> {
    if arguments.pr == 0 {
        bail!("PR number must be positive");
    }
    let github = GitHub::new(&arguments.repo, &env::var("GH_TOKEN").unwrap_or_default())?;
    let required: Vec<String> = serde_json::from_str(&env::var("REQUIRED_CHECKS")?)?;
    let enabled = reviews::review_pr(
        &github,
        arguments.pr,
        &required,
        env::var("RADY_MODEL")
            .ok()
            .filter(|value| !value.is_empty())
            .as_deref(),
        &env::var("APP_SLUG").unwrap_or_default(),
        arguments.harness,
        &env::var("SCORE").unwrap_or_default(),
        &env::var("UPDATE_TYPE").unwrap_or_default(),
        &env::var("MAINTAINER_CHANGES").unwrap_or_default(),
        &env::var("EXPECTED_HEAD").unwrap_or_default(),
        Duration::from_secs(180),
    )?;
    action_output(&[("enable_auto_merge", enabled.to_string())])
}

fn action_output(values: &[(&str, String)]) -> Result<()> {
    if let Some(path) = env::var_os("GITHUB_OUTPUT") {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        for (key, value) in values {
            writeln!(file, "{key}={value}")?;
        }
    } else {
        let value = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), json!(value)))
            .collect::<serde_json::Map<_, _>>();
        println!("{}", serde_json::to_string(&value)?);
    }
    Ok(())
}

fn deduplicate(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_matrix_covers_unified_commands_themes_and_agent_output() {
        for arguments in [
            vec!["rady", "--help"],
            vec!["rady", "code", "--help"],
            vec!["rady", "dependasolve", "--help"],
            vec!["rady", "runs", "--help"],
            vec!["rady", "inspect", "--help"],
            vec!["rady", "cancel", "--help"],
            vec!["rady", "resume", "--help"],
            vec!["rady", "apply", "--help"],
            vec![
                "rady", "--theme", "tide", "--output", "json", "doctor", "--help",
            ],
        ] {
            let result = Cli::try_parse_from(arguments);
            assert!(result.is_err_and(|error| error.exit_code() == 0));
        }
    }

    #[test]
    fn deduplication_preserves_first_seen_order() {
        assert_eq!(
            deduplicate(["test".to_owned(), "lint".to_owned(), "test".to_owned()]),
            ["test", "lint"]
        );
    }

    #[test]
    fn dependasolve_defaults_the_solver_source() {
        let cli = Cli::try_parse_from([
            "rady",
            "dependasolve",
            "--repo",
            "owner/repo",
            "--checks",
            "test",
        ])
        .expect("solver-ref must be optional");
        let Commands::Dependasolve(arguments) = cli.command else {
            panic!("dependasolve command expected");
        };
        assert!(arguments.solver_ref.is_none());
        assert!(!arguments.no_overwrite);

        let cli = Cli::try_parse_from([
            "rady",
            "dependasolve",
            "--repo",
            "owner/repo",
            "--checks",
            "test",
            "--no-overwrite",
        ])
        .expect("no-overwrite must be accepted");
        let Commands::Dependasolve(arguments) = cli.command else {
            panic!("dependasolve command expected");
        };
        assert!(arguments.no_overwrite);
    }

    #[test]
    fn json_failures_share_one_bounded_contract() {
        for (arguments, kind, expected) in [
            (
                vec!["rady", "--output", "json", "code", "change it"],
                "runtime",
                "code requires a --check",
            ),
            (
                vec![
                    "rady",
                    "--output",
                    "json",
                    "dependasolve",
                    "--repo",
                    "owner/repo",
                    "--solver-ref",
                    "invalid",
                    "--checks",
                    "test",
                ],
                "runtime",
                "--solver-ref requires",
            ),
            (
                vec!["rady", "dependasolve", "--output=json"],
                "usage",
                "required arguments",
            ),
        ] {
            let error = run_from(arguments).expect_err("invalid input must fail");
            let failure = error
                .downcast_ref::<CliFailure>()
                .expect("JSON failure must retain its output mode");
            assert_eq!(failure.output, OutputMode::Json);
            let document: Value =
                serde_json::from_str(&json_error_document(failure.kind, &failure.message))
                    .expect("failure must be valid JSON");
            assert_eq!(document["schema"], 1);
            assert_eq!(document["status"], "error");
            assert_eq!(document["error"]["kind"], kind);
            assert!(
                document["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(expected))
            );
        }
        let long = anyhow!("{}", "x".repeat(MAX_ERROR_CHARACTERS + 1));
        let failure = CliFailure::new(OutputMode::Json, &long);
        assert!(failure.message.ends_with("[Error truncated]"));
        assert!(
            failure.message.chars().count()
                <= MAX_ERROR_CHARACTERS + "\n[Error truncated]".chars().count()
        );
    }
}
