use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

use crate::Result;
use crate::agent::{self, Harness};
use crate::apps::Identity;
use crate::code_command::CodeArgs;
use crate::delivery;
use crate::github::GitHub;
use crate::repair;
use crate::reviews;
use crate::session::{self, SessionAnswer, SessionConfig};
use crate::setup::{self, SourceRef};
use crate::ui::{OutputMode, Theme, Ui, json_success_document, print_markdown};

#[derive(Debug, Parser)]
#[command(
    name = "rady",
    version,
    about = "Human-first, evidence-gated coding and dependency review",
    disable_help_subcommand = true,
    after_help = "Examples:\n  rady setup\n  rady code \"add structured logging\" --check test\n  rady agent ask \"why is this test failing?\"\n  rady dependasolve --repo owner/repo --check test --apply"
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
    /// Connect this repository to radyybot
    #[command(after_help = "Example:\n  rady setup")]
    Setup(SetupArgs),

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
        after_help = "Examples:\n  rady agent ask \"how does this parser work?\"\n  rady agent follow-up RUN_ID \"where is that called?\"\n  rady agent serve --app-client-id CLIENT_ID --app-private-key-file KEY.pem\n  rady agent doctor"
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
    Serve(crate::service::ServeArgs),

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
    Sweep(crate::service::SweepArgs),

    #[command(hide = true)]
    PrepareRepair(PrepareRepairArgs),
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
struct SetupArgs {
    /// Repository to connect; defaults to the current GitHub repository
    #[arg(long)]
    repo: Option<String>,

    /// Required CI check; defaults to evidence from the repository
    #[arg(long = "check", visible_alias = "checks", num_args = 1..)]
    checks: Vec<String>,

    #[arg(long, default_value = ".")]
    directory: PathBuf,

    /// Optional trusted solver source; defaults to the latest keys-i/rady commit
    #[arg(long)]
    solver_ref: Option<String>,

    /// Refuse to replace an existing generated configuration
    #[arg(long)]
    no_overwrite: bool,

    /// Accept the current Rady service terms and privacy policy
    #[arg(long)]
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
            eprintln!("Rady couldn't finish: {}", failure.message);
        }
        return ExitCode::from(failure.exit_code);
    } else {
        eprintln!("Rady couldn't finish: {error:#}");
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
        Commands::Setup(arguments) => setup(arguments, cli.theme, cli.output),
        Commands::Code(arguments) => crate::code_command::run(*arguments, cli.theme, cli.output),
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
            AgentCommands::Serve(arguments) => crate::service::serve(arguments),
            AgentCommands::Run(arguments) => native_agent(arguments),
            AgentCommands::Doctor(arguments) => doctor(arguments, cli.theme, cli.output),
            AgentCommands::Resolve(arguments) => resolve(arguments),
            AgentCommands::Review(arguments) => review(arguments),
            AgentCommands::Respond(arguments) => respond(arguments),
            AgentCommands::Sweep(arguments) => crate::service::sweep(arguments),
            AgentCommands::PrepareRepair(arguments) => prepare_repair(arguments),
        },
        Commands::Resolve(arguments) => resolve(arguments),
        Commands::Review(arguments) => review(arguments),
    };
    result.map_err(|error| CliFailure::new(output, &error).into())
}

fn setup(mut arguments: SetupArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let mut ui = Ui::new(theme, output, 7);
    ui.title(
        "Rady setup",
        "Connect this repository without copying secrets into it",
    );
    ui.stage("Finding the repository");
    let repository = setup::resolve_repository_in(arguments.repo.as_deref(), &arguments.directory)?;
    ui.stage("Finding a check to run");
    let checks = setup::resolve_checks(&repository, &arguments.checks)?;
    ui.stage("Checking the service agreement");
    let agreement_exists = setup::has_verified_agreement(&repository, &arguments.directory)?;
    if arguments.accept_terms || !agreement_exists {
        ui.finish_progress();
        arguments.accept_terms =
            accept_terms(&repository, &checks, theme, output, arguments.accept_terms)?;
    } else {
        arguments.accept_terms = true;
    }
    ui.stage(if agreement_exists {
        "Agreement already on file"
    } else {
        "Agreement accepted"
    });
    ui.stage("Pinning the trusted Rady version");
    let source = SourceRef::resolve(arguments.solver_ref.as_deref())?;
    ui.stage("Connecting radyybot and saving the setup");
    ui.finish_progress();
    let preview = setup::run(
        &repository,
        &source,
        &checks,
        &arguments.directory,
        Identity::Rady,
        false,
        !arguments.no_overwrite,
        true,
        arguments.accept_terms,
    )?;
    ui.stage("Ready to review");
    if output == OutputMode::Json {
        println!("{}", json_success_document("setup", &preview)?);
        return Ok(());
    }
    let files = preview["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|path| format!("- `{path}`"))
        .collect::<Vec<_>>()
        .join("\n");
    print_markdown(
        &format!(
            "## Repository is ready\n\n**Repository:** `{repository}`\n\n**Checks:** {}\n\n**Credentials stay in:** `keys-i/rady`\n\nNo secrets were added here. If GitHub opened the radyybot installation page, finish it, then commit the generated files below. The service will handle mentions and dependency pull requests on its next pass.\n\n### Files\n\n{files}",
            checks.join(", ")
        ),
        theme,
    )
}

fn accept_terms(
    repository: &str,
    checks: &[String],
    theme: Theme,
    output: OutputMode,
    accepted: bool,
) -> Result<bool> {
    if output == OutputMode::Json
        || !io::stdin().is_terminal()
        || !io::stdout().is_terminal()
        || !io::stderr().is_terminal()
    {
        if accepted {
            return Ok(true);
        }
        bail!(
            "read {} and {}, then rerun with --accept-terms if you agree",
            setup::TERMS_URL,
            setup::PRIVACY_URL
        );
    }
    print_markdown(&setup_consent_preview(repository, checks), theme)?;
    if accepted {
        eprintln!("Accepted with --accept-terms. Continuing with {repository}.\n");
        return Ok(true);
    }
    eprint!("Accept and continue? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Ok(true);
    }
    bail!("setup was not changed; rerun with --accept-terms after you agree")
}

fn setup_consent_preview(repository: &str, checks: &[String]) -> String {
    format!(
        "## Before radyybot connects\n\nFor `{repository}`, Rady will:\n\n- verify your admin access and open the radyybot installation page if needed\n- use `{}` as CI evidence\n- read relevant issues, pull requests, diffs and check results\n- record your agreement in a closed issue and non-secret `.github/rady.json` file\n- add Dependabot configuration only when it is missing\n- send bounded evidence to the model providers described in the privacy policy\n\nYour App and model credentials stay in `keys-i/rady`. Rady won't copy them here or change branch protection. You still decide what gets merged.\n\n**Terms:** {}\n\n**Privacy:** {}\n",
        checks.join("`, `"),
        setup::TERMS_URL,
        setup::PRIVACY_URL,
    )
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

pub(crate) fn print_session(
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

fn dependasolve(arguments: DependSolveArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let mut ui = Ui::new(theme, output, 3);
    ui.title(
        "Rady dependasolve",
        "Review dependency updates with the checks you trust",
    );
    ui.stage("Finding Rady's source");
    let source = SourceRef::resolve(arguments.solver_ref.as_deref())?;
    ui.stage("Checking repository setup");
    ui.finish_progress();
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
        "Repository ready"
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
                "Repository is ready"
            } else {
                "Setup preview"
            },
            arguments.repo,
            source.joined(),
            arguments.checks.join(", "),
            files,
            if arguments.apply {
                "Your agreement is saved. radyybot will check its access, then handle mentions and pending dependency pull requests."
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
    ui.title("Rady doctor", "Check what's ready on this machine");
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
        ui.success("This machine is ready for local coding");
        ui.note("Opening a pull request also needs push access and fixed acceptance checks");
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
    command
        .env_clear()
        .envs(agent::safe_environment())
        .args(prefix)
        .args(arguments.arguments);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_matrix_covers_human_commands_and_help_without_help_subcommands() {
        for arguments in [
            vec!["rady", "--help"],
            vec!["rady", "setup", "--help"],
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

        let help = Cli::try_parse_from(["rady", "setup", "--help"])
            .expect_err("help exits after rendering")
            .to_string();
        assert!(
            !help.contains("app-client-id"),
            "setup keeps App keys central"
        );
        assert!(
            !help.contains("private-key") && !help.contains("gemini") && !help.contains("cerebras"),
            "setup must not ask target repositories for provider credentials"
        );
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
            vec!["rady", "setup", "--repo", "owner/repo", "--check", "test"],
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
            vec![
                "rady",
                "agent",
                "serve",
                "--owner",
                "keys-i",
                "--app-client-id",
                "Iv1.abc",
                "--app-private-key-file",
                "/secure/radyybot.pem",
                "--once",
            ],
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
    fn setup_requires_explicit_consent_for_json_output() {
        let error = accept_terms(
            "owner/repo",
            &["test".to_owned()],
            Theme::Plain,
            OutputMode::Json,
            false,
        )
        .expect_err("JSON setup cannot prompt");
        let message = error.to_string();
        assert!(message.contains(setup::TERMS_URL));
        assert!(message.contains(setup::PRIVACY_URL));
        assert!(message.contains("--accept-terms"));
        assert!(
            accept_terms(
                "owner/repo",
                &["test".to_owned()],
                Theme::Plain,
                OutputMode::Json,
                true,
            )
            .unwrap()
        );

        let preview = setup_consent_preview("owner/repo", &["test".to_owned()]);
        for expected in [
            "owner/repo",
            "`test` as CI evidence",
            ".github/rady.json",
            "closed issue",
            "keys-i/rady",
            setup::TERMS_URL,
            setup::PRIVACY_URL,
        ] {
            assert!(preview.contains(expected), "{expected}: {preview}");
        }
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
