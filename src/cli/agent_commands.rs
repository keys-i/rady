use std::env;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde_json::json;

use crate::Result;
use crate::agent::{self, Harness};
use crate::github::GitHub;
use crate::reviews;
use crate::reviews::repair;
use crate::ui::{OutputMode, Theme, Ui};

use super::{AgentArgs, DoctorArgs, PrepareRepairArgs, ResolveArgs, RespondArgs, ReviewArgs};

pub(super) fn doctor(arguments: DoctorArgs, theme: Theme, output: OutputMode) -> Result<()> {
    let mut ui = Ui::new(theme, output, 3);
    ui.title("Pekin doctor", "Check what's ready on this machine");
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

pub(super) fn native_agent(arguments: AgentArgs) -> Result<()> {
    let (program, prefix) = if arguments.harness == Harness::Command {
        let configured =
            agent::split_command(&env::var("PEKIN_AGENT_COMMAND").unwrap_or_default())?;
        let (program, prefix) = configured
            .split_first()
            .ok_or_else(|| anyhow!("set PEKIN_AGENT_COMMAND"))?;
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

pub(super) fn resolve(arguments: ResolveArgs) -> Result<()> {
    if arguments.pr == 0 {
        bail!("PR number must be positive");
    }
    let github = GitHub::new(&arguments.repo, &env::var("GH_TOKEN").unwrap_or_default())?;
    let (pull, dependency, metadata) = reviews::resolve(&github, arguments.pr)?;
    action_output(&[
        ("dependency", dependency.to_string()),
        (
            "head",
            pull["head"]["sha"].as_str().unwrap_or_default().to_owned(),
        ),
        (
            "update_type",
            metadata
                .as_ref()
                .map(|value| value.update_type.clone())
                .unwrap_or_default(),
        ),
        (
            "maintainer_changes",
            metadata.map_or_else(|| "unknown".to_owned(), |value| value.maintainer_changes),
        ),
    ])
}

pub(super) fn review(arguments: ReviewArgs) -> Result<()> {
    if arguments.pr == 0 {
        bail!("PR number must be positive");
    }
    let github = GitHub::new(&arguments.repo, &env::var("GH_TOKEN").unwrap_or_default())?;
    let required: Vec<String> = serde_json::from_str(&env::var("REQUIRED_CHECKS")?)?;
    let outcome = reviews::review_pr(
        &github,
        arguments.pr,
        &required,
        env::var("PEKIN_MODEL")
            .ok()
            .filter(|value| !value.is_empty())
            .as_deref(),
        &env::var("APP_SLUG").unwrap_or_default(),
        arguments.harness,
        match env::var("PEKIN_REPOSITORY_PRIVATE").ok().as_deref() {
            Some("true") => Some(true),
            Some("false") => Some(false),
            _ => None,
        },
        &env::var("UPDATE_TYPE").unwrap_or_default(),
        &env::var("MAINTAINER_CHANGES").unwrap_or_default(),
        &env::var("EXPECTED_HEAD").unwrap_or_default(),
        Duration::from_secs(180),
    )?;
    action_output(&[("approved", outcome.approved.to_string())])
}

pub(super) fn respond(arguments: RespondArgs) -> Result<()> {
    if arguments.issue == 0 || arguments.comment == 0 {
        bail!("issue and comment numbers must be positive");
    }
    let github = GitHub::new(&arguments.repo, &env::var("GH_TOKEN").unwrap_or_default())?;
    crate::mentions::respond(
        &github,
        arguments.issue,
        arguments.comment,
        env::var("PEKIN_MODEL")
            .ok()
            .filter(|value| !value.is_empty())
            .as_deref(),
        arguments.harness,
    )
}

pub(super) fn prepare_repair(arguments: PrepareRepairArgs) -> Result<()> {
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
