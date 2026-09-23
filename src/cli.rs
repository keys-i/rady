use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

use crate::Result;
use crate::agent::{self, Harness};
use crate::apps::Identity;
use crate::delivery::{self, Config};
use crate::github::{self, GitHub};
use crate::quality;
use crate::repair;
use crate::reviews;
use crate::routing::Intent;
use crate::session::{self, SessionAnswer, SessionConfig};
use crate::setup::{self, SourceRef};
use crate::ui::{OutputMode, Theme, Ui, json_success_document, print_markdown};

#[derive(Debug, Parser)]
#[command(
    name = "rady",
    version,
    about = "Human-first, evidence-gated coding and dependency review",
    disable_help_subcommand = true,
    after_help = "Examples:\n  rady code \"add structured logging\" --check test\n  rady agent ask \"why is this test failing?\"\n  rady dependasolve --repo owner/repo --check test --apply"
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
    #[command(after_help = "Example:\n  rady code \"fix the parser\" --check test")]
    Code(Box<CodeArgs>),

    /// Configure Rady's Dependabot review and merge gates
    #[command(
        alias = "dependasolver",
        after_help = "Example:\n  rady dependasolve --repo owner/repo --check test --apply"
    )]
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
    #[command(hide = true)]
    Doctor(DoctorArgs),

    /// Run or check a native agent harness
    #[command(
        disable_help_subcommand = true,
        after_help = "Examples:\n  rady agent ask \"how does this parser work?\"\n  rady agent follow-up RUN_ID \"where is that called?\"\n  rady agent serve\n  rady agent doctor"
    )]
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },

    #[command(hide = true)]
    Resolve(ResolveArgs),

    #[command(hide = true)]
    Review(ReviewArgs),
}

#[derive(Debug, Subcommand)]
enum AgentCommands {
    /// Answer a repository question without creating an edit workspace
    Ask(AskArgs),

    /// Continue a retained repository conversation
    FollowUp(FollowUpArgs),

    /// Keep mention and pull-request review loops running
    Serve(ServeArgs),

    /// Pass arguments to a native agent harness unchanged
    Run(AgentArgs),

    /// Check native harness logins and delivery tools
    Doctor(DoctorArgs),

    #[command(hide = true)]
    Resolve(ResolveArgs),

    #[command(hide = true)]
    Review(ReviewArgs),

    #[command(hide = true)]
    Respond(RespondArgs),

    #[command(hide = true)]
    Sweep(SweepArgs),

    #[command(hide = true)]
    PrepareRepair(PrepareRepairArgs),
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

    /// Enable a named MCP server from .rady/context.json for write tasks
    #[arg(long = "mcp", action = clap::ArgAction::Append)]
    mcp_servers: Vec<String>,

    /// Keep remote delivery as one final commit instead of task checkpoints
    #[arg(long)]
    ghost: bool,
}

#[derive(Debug, Args)]
struct AskArgs {
    question: String,

    #[arg(long, default_value = ".")]
    directory: PathBuf,

    #[arg(long)]
    model: Option<String>,

    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,

    #[arg(long, default_value_t = 300)]
    timeout: u64,
}

#[derive(Debug, Args)]
struct FollowUpArgs {
    run: String,
    question: String,

    #[arg(long)]
    model: Option<String>,

    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,

    #[arg(long, default_value_t = 300)]
    timeout: u64,
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Restrict the service to one installed account
    #[arg(long)]
    owner: Option<String>,

    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,

    #[arg(long, default_value_t = 30)]
    interval: u64,

    #[arg(long, default_value_t = 4)]
    max_reviews: usize,

    /// Privileged local command that prints a fresh App installation token
    #[arg(long, env = "RADY_APP_TOKEN_COMMAND")]
    token_command: Option<String>,

    #[arg(long)]
    once: bool,
}

#[derive(Debug, Args)]
struct DependSolveArgs {
    #[arg(long)]
    repo: String,

    /// Optional trusted solver source; defaults to the latest keys-i/rady commit
    #[arg(long)]
    solver_ref: Option<String>,

    #[arg(long = "check", visible_alias = "checks", required = true, num_args = 1..)]
    checks: Vec<String>,

    #[arg(long, default_value = ".")]
    directory: PathBuf,

    #[arg(long = "app", value_enum, default_value = "rady")]
    identity: Identity,

    #[arg(long)]
    new_app: bool,

    /// Refuse to replace an existing generated workflow
    #[arg(long)]
    no_overwrite: bool,

    #[arg(long)]
    apply: bool,

    /// Accept the current Rady service terms and privacy policy for this repository
    #[arg(long, requires = "apply")]
    accept_terms: bool,
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

#[derive(Debug, Args)]
struct RespondArgs {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    issue: u64,
    #[arg(long)]
    comment: u64,
    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,
}

#[derive(Debug, Args)]
struct SweepArgs {
    #[arg(long)]
    owner: Option<String>,
    #[arg(long, value_enum, env = "RADY_HARNESS", default_value = "codex")]
    harness: Harness,
}

#[derive(Debug, Args)]
struct PrepareRepairArgs {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    pr: u64,
    #[arg(long)]
    expected_head: String,
    #[arg(long)]
    expected_base: String,
    #[arg(long)]
    output: PathBuf,
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
        Commands::Agent { command } => match command {
            AgentCommands::Ask(arguments) => ask(arguments, cli.theme, cli.output),
            AgentCommands::FollowUp(arguments) => follow_up(arguments, cli.theme, cli.output),
            AgentCommands::Serve(arguments) => serve(arguments),
            AgentCommands::Run(arguments) => native_agent(arguments),
            AgentCommands::Doctor(arguments) => doctor(arguments, cli.theme, cli.output),
            AgentCommands::Resolve(arguments) => resolve(arguments),
            AgentCommands::Review(arguments) => review(arguments),
            AgentCommands::Respond(arguments) => respond(arguments),
            AgentCommands::Sweep(arguments) => sweep(arguments),
            AgentCommands::PrepareRepair(arguments) => prepare_repair(arguments),
        },
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
    let conversational = arguments.spec.is_none()
        && !arguments.pr
        && arguments.repo.is_none()
        && arguments.base.is_none()
        && arguments.expected_start.is_none()
        && arguments.checks.is_empty()
        && arguments.benchmarks.is_empty()
        && arguments.mcp_servers.is_empty()
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
            return print_session("ask", &answer, theme, output);
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
        mcp_servers: arguments.mcp_servers,
        ghost: arguments.ghost,
        theme,
        output,
    })?;
    Ok(())
}

fn ask(arguments: AskArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let answer = session::ask(
        &arguments.question,
        &SessionConfig {
            directory: arguments.directory,
            harness: arguments.harness,
            model: arguments.model,
            timeout: Duration::from_secs(arguments.timeout),
        },
    )?;
    print_session("ask", &answer, theme, output)
}

fn follow_up(arguments: FollowUpArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let answer = session::follow_up(
        &arguments.run,
        &arguments.question,
        &SessionConfig {
            directory: PathBuf::from("."),
            harness: arguments.harness,
            model: arguments.model,
            timeout: Duration::from_secs(arguments.timeout),
        },
    )?;
    print_session("follow_up", &answer, theme, output)
}

fn print_session(
    kind: &str,
    answer: &SessionAnswer,
    theme: Theme,
    output: OutputMode,
) -> Result<()> {
    if output == OutputMode::Json {
        println!(
            "{}",
            json_success_document(kind, &serde_json::to_value(answer)?)?
        );
        return Ok(());
    }
    let mut markdown = format!("{}\n", answer.answer);
    if !answer.follow_ups.is_empty() {
        markdown.push_str("\nYou could ask next:\n\n");
        for question in &answer.follow_ups {
            markdown.push_str(&format!("- {}\n", question));
        }
    }
    markdown.push_str(&format!(
        "\nContinue with `rady agent follow-up {} \"…\"`.\n",
        answer.id
    ));
    print_markdown(&markdown, theme)
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
        arguments.accept_terms,
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
            "## {}\n\n**Repository:** `{}`\n\n**Source:** `{}`\n\n**CI evidence:** {}\n\n**Branch protection:** unchanged\n\n### Files\n\n{}\n\n{}",
            if arguments.apply {
                "Setup complete"
            } else {
                "Setup preview"
            },
            arguments.repo,
            source.joined(),
            arguments.checks.join(", "),
            files,
            if arguments.apply {
                "Consent is recorded. Central orchestration will verify radyybot access before picking up mentions and pending pull requests."
            } else {
                "Read `docs/TERMS.md` and `docs/PRIVACY.md`, then run again with `--apply --accept-terms` if you agree."
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
        "RADY_GEMINI_API_KEY",
        "RADY_CEREBRAS_API_KEY",
        "RADY_XAI_API_KEY",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "RADY_PUSH_TOKEN",
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
    let outcome = reviews::review_pr(
        &github,
        arguments.pr,
        &required,
        env::var("RADY_MODEL")
            .ok()
            .filter(|value| !value.is_empty())
            .as_deref(),
        &env::var("APP_SLUG").unwrap_or_default(),
        arguments.harness,
        match env::var("RADY_REPOSITORY_PRIVATE").ok().as_deref() {
            Some("true") => Some(true),
            Some("false") => Some(false),
            _ => None,
        },
        &env::var("SCORE").unwrap_or_default(),
        &env::var("UPDATE_TYPE").unwrap_or_default(),
        &env::var("MAINTAINER_CHANGES").unwrap_or_default(),
        &env::var("EXPECTED_HEAD").unwrap_or_default(),
        Duration::from_secs(180),
    )?;
    action_output(&[
        ("approved", outcome.approved.to_string()),
        ("enable_auto_merge", outcome.enable_auto_merge.to_string()),
    ])
}

fn respond(arguments: RespondArgs) -> Result<()> {
    if arguments.issue == 0 || arguments.comment == 0 {
        bail!("issue and comment numbers must be positive");
    }
    let github = GitHub::new(&arguments.repo, &env::var("GH_TOKEN").unwrap_or_default())?;
    crate::mentions::respond(
        &github,
        arguments.issue,
        arguments.comment,
        env::var("RADY_MODEL")
            .ok()
            .filter(|value| !value.is_empty())
            .as_deref(),
        arguments.harness,
    )
}

const MAX_SWEEP_FAILURES: usize = 8;
const SERVICE_TOKEN_ENVIRONMENT: &[&str] = &[
    "RADY_APP_PRIVATE_KEY",
    "RADY_APP_CLIENT_ID",
    "RADY_APP_ID",
    "RADY_APP_SLUG",
];

fn sweep(arguments: SweepArgs) -> Result<()> {
    let token = env::var("GH_TOKEN").unwrap_or_default();
    sweep_with_token(&arguments, &token, &mut BTreeMap::new())
}

fn sweep_with_token(
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
    bail!(
        "central sweep had {} failure(s): {}",
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

fn record_sweep_failure(failures: &mut Vec<String>, repo: &str, error: &anyhow::Error) {
    if failures.len() < MAX_SWEEP_FAILURES {
        failures.push(format!("{repo}: {error}"));
    }
}

fn validate_owner_filter(owner: Option<&str>) -> Result<()> {
    if let Some(owner) = owner {
        github::validate_repository(&format!("{owner}/rady"))?;
    }
    Ok(())
}

fn owner_matches(repository: &Value, owner: Option<&str>) -> bool {
    owner.is_none_or(|owner| {
        repository["owner"]["login"]
            .as_str()
            .is_some_and(|login| login.eq_ignore_ascii_case(owner))
    })
}

fn serve(arguments: ServeArgs) -> Result<()> {
    validate_owner_filter(arguments.owner.as_deref())?;
    if !(5..=3_600).contains(&arguments.interval) || !(1..=16).contains(&arguments.max_reviews) {
        bail!("use a service interval from 5 to 3600 seconds and 1-16 reviews per cycle");
    }
    let mut consecutive_failures = 0_u8;
    let mut mention_cursors = BTreeMap::new();
    loop {
        let cycle = service_cycle(&arguments, &mut mention_cursors);
        if let Ok(reviewed) = &cycle {
            consecutive_failures = 0;
            eprintln!(
                "Rady service cycle complete · {} new pull request review(s)",
                reviewed
            );
        } else {
            consecutive_failures = consecutive_failures.saturating_add(1);
            eprintln!(
                "Rady service cycle needs attention: {}",
                cycle.as_ref().unwrap_err()
            );
        }
        if arguments.once {
            return cycle.map(|_| ());
        }
        if consecutive_failures >= 3 {
            bail!("persistent service stopped after three failed cycles");
        }
        thread::sleep(Duration::from_secs(arguments.interval));
    }
}

fn service_cycle(
    arguments: &ServeArgs,
    mention_cursors: &mut BTreeMap<String, u64>,
) -> Result<usize> {
    let token = service_token(arguments)?;
    let mentions = sweep_with_token(
        &SweepArgs {
            owner: arguments.owner.clone(),
            harness: arguments.harness,
        },
        &token,
        mention_cursors,
    );
    let reviews = service_reviews(arguments, &token);
    match (mentions, reviews) {
        (Ok(()), Ok(reviewed)) => Ok(reviewed),
        (Err(mention), Ok(_)) => Err(mention),
        (Ok(()), Err(review)) => Err(review),
        (Err(mention), Err(review)) => {
            bail!("mention sweep: {mention}; pull-request sweep: {review}")
        }
    }
}

fn service_token(arguments: &ServeArgs) -> Result<String> {
    let token = if let Some(command) = arguments.token_command.as_deref() {
        let parts = agent::split_command(command)?;
        let (program, arguments) = parts
            .split_first()
            .ok_or_else(|| anyhow!("token command is empty"))?;
        let program = agent::which(program)
            .ok_or_else(|| anyhow!("token command executable is not installed"))?;
        let output = agent::execute(
            program.as_os_str(),
            arguments,
            Path::new("."),
            b"",
            Duration::from_secs(30),
            &service_token_environment(),
            false,
            None,
        )?;
        if output.code != 0 {
            bail!("token command failed; no GitHub request was made");
        }
        output.stdout.trim().to_owned()
    } else {
        env::var("GH_TOKEN").unwrap_or_default()
    };
    if token.is_empty() || token.len() > 8_192 || !token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        bail!("provide a valid App installation token or --token-command");
    }
    Ok(token)
}

fn service_token_environment() -> std::collections::BTreeMap<String, String> {
    SERVICE_TOKEN_ENVIRONMENT
        .iter()
        .copied()
        .filter_map(|name| env::var(name).ok().map(|value| (name.to_owned(), value)))
        .collect()
}

fn service_reviews(arguments: &ServeArgs, token: &str) -> Result<usize> {
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
        let github = GitHub::new(name, token)?;
        let configuration = match github
            .raw_optional("contents/.github/rady.json")?
            .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        {
            Some(configuration) if setup::verified_configuration(&github, &configuration)? => {
                configuration
            }
            _ => continue,
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
            if reviewed >= arguments.max_reviews {
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
    if failures.is_empty() {
        Ok(reviewed)
    } else {
        bail!(
            "persistent review had {} failure(s): {}",
            failures.len(),
            failures.join("; ")
        )
    }
}

fn prepare_repair(arguments: PrepareRepairArgs) -> Result<()> {
    if arguments.pr == 0 {
        bail!("PR number must be positive");
    }
    let github = GitHub::new(&arguments.repo, &env::var("GH_TOKEN").unwrap_or_default())?;
    repair::prepare(
        &github,
        arguments.pr,
        &arguments.expected_head,
        &arguments.expected_base,
        &arguments.output,
    )
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
    fn cli_matrix_covers_human_commands_and_help_without_help_subcommands() {
        for arguments in [
            vec!["rady", "--help"],
            vec!["rady", "code", "--help"],
            vec!["rady", "dependasolve", "--help"],
            vec!["rady", "agent", "--help"],
            vec!["rady", "agent", "run", "--help"],
            vec!["rady", "agent", "doctor", "--help"],
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

        for arguments in [vec!["rady", "help"], vec!["rady", "agent", "help"]] {
            let result = Cli::try_parse_from(arguments);
            assert!(result.is_err_and(|error| error.exit_code() != 0));
        }

        for arguments in [vec!["rady", "--help"], vec!["rady", "agent", "--help"]] {
            let help = Cli::try_parse_from(arguments)
                .expect_err("help exits after rendering")
                .to_string();
            assert!(
                !help
                    .lines()
                    .any(|line| line.trim_start().starts_with("help")),
                "help must not be a generated subcommand: {help}"
            );
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
    fn code_accepts_the_hidden_pinned_start_revision() {
        let expected_start = "a".repeat(40);
        let cli = Cli::try_parse_from([
            "rady".to_owned(),
            "code".to_owned(),
            "repair dependency conflict".to_owned(),
            "--expected-start".to_owned(),
            expected_start.clone(),
        ])
        .expect("workflow revision must parse");
        let Commands::Code(arguments) = cli.command else {
            panic!("code command expected");
        };
        assert_eq!(
            arguments.expected_start.as_deref(),
            Some(expected_start.as_str())
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

    #[test]
    fn parser_accepts_agent_hierarchy_and_dependasolve_check_aliases() {
        for arguments in [
            vec![
                "rady",
                "dependasolve",
                "--repo",
                "owner/repo",
                "--check",
                "test",
            ],
            vec![
                "rady",
                "dependasolve",
                "--repo",
                "owner/repo",
                "--checks",
                "test",
                "lint",
            ],
            vec!["rady", "agent", "run", "--", "exec", "--help"],
            vec!["rady", "agent", "doctor"],
            vec!["rady", "agent", "ask", "What changed?"],
            vec![
                "rady",
                "agent",
                "follow-up",
                "run_0123456789abcdef0123456789abcdef",
                "Why?",
            ],
            vec!["rady", "agent", "serve", "--once"],
            vec![
                "rady",
                "agent",
                "resolve",
                "--repo",
                "owner/repo",
                "--pr",
                "1",
            ],
            vec![
                "rady",
                "agent",
                "review",
                "--repo",
                "owner/repo",
                "--pr",
                "1",
            ],
            vec![
                "rady",
                "agent",
                "respond",
                "--repo",
                "owner/repo",
                "--issue",
                "1",
                "--comment",
                "2",
            ],
            vec!["rady", "agent", "sweep", "--owner", "keys-i"],
            vec![
                "rady",
                "code",
                "fix it",
                "--check",
                "test",
                "--pr",
                "--repo",
                "owner/repo",
                "--mcp",
                "docs",
                "--ghost",
            ],
        ] {
            Cli::try_parse_from(arguments).expect("command must parse");
        }

        let cli = Cli::try_parse_from([
            "rady",
            "dependasolve",
            "--repo",
            "owner/repo",
            "--check",
            "test",
            "--no-overwrite",
        ])
        .expect("solver-ref must be optional");
        let Commands::Dependasolve(arguments) = cli.command else {
            panic!("dependasolve command expected");
        };
        assert!(arguments.solver_ref.is_none());
        assert!(arguments.no_overwrite);
        assert_eq!(arguments.identity, Identity::Rady);
    }

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
    fn service_boundaries_are_narrow_and_cross_owner() {
        let repository = json!({"owner": {"login": "Keys-I"}});
        assert!(owner_matches(&repository, None));
        assert!(owner_matches(&repository, Some("keys-i")));
        assert!(!owner_matches(&repository, Some("other")));
        assert!(validate_owner_filter(Some("keys-i")).is_ok());
        assert!(validate_owner_filter(Some("bad owner")).is_err());
        assert!(SERVICE_TOKEN_ENVIRONMENT.contains(&"RADY_APP_PRIVATE_KEY"));
        assert!(!SERVICE_TOKEN_ENVIRONMENT.contains(&"RADY_GEMINI_API_KEY"));
        assert!(!SERVICE_TOKEN_ENVIRONMENT.contains(&"GH_TOKEN"));
    }

    #[test]
    fn json_failures_share_one_bounded_contract() {
        for (arguments, kind, expected) in [
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
