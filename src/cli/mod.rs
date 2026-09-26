use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde_json::json;

use crate::Result;
use crate::agent::Harness;
use crate::agent::session::{self, SessionAnswer, SessionConfig};
use crate::delivery;
use crate::ui::{OutputMode, Theme, json_success_document, print_markdown};

mod agent_commands;
mod code;
mod repository_setup;

use agent_commands::{doctor, native_agent, prepare_repair, resolve, respond, review};
use code::CodeArgs;
use repository_setup::{dependasolve, setup};

#[derive(Debug, Parser)]
#[command(
    name = "koelu",
    version,
    about = "Human-first, evidence-gated coding and dependency review",
    disable_help_subcommand = true,
    after_help = "Examples:\n  koelu setup\n  koelu code \"add structured logging\" --check test\n  koelu agent ask \"why is this test failing?\"\n  koelu dependasolve --repo owner/repo --check test --apply"
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
    /// Connect this repository to Koelu
    #[command(after_help = "Example:\n  koelu setup")]
    Setup(SetupArgs),

    /// Turn a request into a checked local change or pull request
    #[command(after_help = "Example:\n  koelu code \"fix the parser\" --check test")]
    Code(Box<CodeArgs>),

    /// Configure Koelu's Dependabot review
    #[command(after_help = "Example:\n  koelu dependasolve --repo owner/repo --check test --apply")]
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

    /// Run or check a native agent harness
    #[command(
        disable_help_subcommand = true,
        after_help = "Examples:\n  koelu agent ask \"how does this parser work?\"\n  koelu agent follow-up RUN_ID \"where is that called?\"\n  koelu agent serve --app-client-id CLIENT_ID --app-private-key-file KEY.pem\n  koelu agent doctor"
    )]
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
}

#[derive(Debug, Subcommand)]
enum AgentCommands {
    /// Answer a repository question without creating an edit workspace
    Ask(AskArgs),

    /// Continue a retained repository conversation
    FollowUp(FollowUpArgs),

    /// Keep the installed-repository mention loop running
    Serve(crate::github::service::ServeArgs),

    #[command(hide = true)]
    Targets(crate::github::service::ServeArgs),

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
    PrepareRepair(PrepareRepairArgs),
}

#[derive(Debug, Args)]
struct AskArgs {
    question: String,

    #[arg(long, default_value = ".")]
    directory: PathBuf,

    #[arg(long)]
    model: Option<String>,

    #[arg(long, value_enum, env = "KOELU_HARNESS", default_value = "codex")]
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

    #[arg(long, value_enum, env = "KOELU_HARNESS", default_value = "codex")]
    harness: Harness,

    #[arg(long, default_value_t = 300)]
    timeout: u64,
}

#[derive(Debug, Args)]
struct DependSolveArgs {
    #[arg(long)]
    repo: String,

    /// Optional trusted solver source; defaults to the latest keys-i/koelu commit
    #[arg(long)]
    solver_ref: Option<String>,

    #[arg(long = "check", visible_alias = "checks", required = true, num_args = 1..)]
    checks: Vec<String>,

    #[arg(long, default_value = ".")]
    directory: PathBuf,

    /// Refuse to replace an existing generated configuration
    #[arg(long)]
    no_overwrite: bool,

    #[arg(long)]
    apply: bool,

    /// Accept the current Koelu service terms and privacy policy for this repository
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

    /// Optional trusted solver source; defaults to the latest keys-i/koelu commit
    #[arg(long)]
    solver_ref: Option<String>,

    /// Refuse to replace an existing generated configuration
    #[arg(long)]
    no_overwrite: bool,

    /// Accept the current Koelu service terms and privacy policy
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
    #[arg(long, value_enum, env = "KOELU_HARNESS", default_value = "codex")]
    harness: Harness,
}

#[derive(Debug, Args)]
struct AgentArgs {
    #[arg(long, value_enum, env = "KOELU_HARNESS", default_value = "codex")]
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
    #[arg(long, value_enum, env = "KOELU_HARNESS", default_value = "codex")]
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
    #[arg(long, value_enum, env = "KOELU_HARNESS", default_value = "codex")]
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
            eprintln!("Koelu couldn't finish: {}", failure.message);
        }
        return ExitCode::from(failure.exit_code);
    } else {
        eprintln!("Koelu couldn't finish: {error:#}");
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
        Commands::Code(arguments) => code::run(*arguments, cli.theme, cli.output),
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
        Commands::Agent { command } => match command {
            AgentCommands::Ask(arguments) => ask(arguments, cli.theme, cli.output),
            AgentCommands::FollowUp(arguments) => follow_up(arguments, cli.theme, cli.output),
            AgentCommands::Serve(arguments) => crate::github::service::serve(arguments),
            AgentCommands::Targets(arguments) => crate::github::service::targets(arguments),
            AgentCommands::Run(arguments) => native_agent(arguments),
            AgentCommands::Doctor(arguments) => doctor(arguments, cli.theme, cli.output),
            AgentCommands::Resolve(arguments) => resolve(arguments),
            AgentCommands::Review(arguments) => review(arguments),
            AgentCommands::Respond(arguments) => respond(arguments),
            AgentCommands::PrepareRepair(arguments) => prepare_repair(arguments),
        },
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
        "\nContinue with `koelu agent follow-up {} \"…\"`.\n",
        answer.id
    ));
    print_markdown(&markdown, theme)
}

#[cfg(test)]
mod tests;
