use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Result;
use crate::agent::{self, Harness, Usage};
use crate::benchmark::{self, Measurement};
use crate::context::{McpConfiguration, RepositoryContext};
use crate::github;
use crate::model::STYLE;
use crate::quality::{self, AcceptanceCheck, Plan, ReviewReport};
use crate::runs::{Run as StoredRun, RunStore};
use crate::ui::{
    OutputMode, ReportState, Theme, Ui, json_success_document, print_markdown, write_report,
};
use anyhow::{anyhow, bail};
use nix::errno::Errno;
use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::Mode;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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

const MAX_PUSH_TOKEN_BYTES: usize = 8_192;

struct GitNetworkAuth {
    environment: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
enum Fingerprint {
    Missing,
    File { mode: u32, sha256: String },
    Symlink { mode: u32, target: String },
}

struct Run {
    scratch: PathBuf,
    stored: StoredRun,
    state: Value,
    usage: Usage,
    ui: Ui,
    theme: Theme,
    output: OutputMode,
}

impl Run {
    fn persist(&self) -> Result<()> {
        self.stored.write_json("run.json", &self.state)?;
        let name = self.state["stage"].as_str().unwrap_or("Rady run");
        write_report(
            &self.scratch.join("run.html"),
            name,
            &report_markdown(&self.state, true),
            match self.state["status"].as_str() {
                Some("ready" | "published" | "applied") => ReportState::Complete,
                Some("stopped" | "cancelled") => ReportState::Stopped,
                _ => ReportState::Active,
            },
            self.theme,
        )
    }

    fn ensure_active(&self) -> Result<()> {
        if self.stored.is_cancelled()? {
            bail!("run cancelled");
        }
        Ok(())
    }

    fn stage(&mut self, name: &str) -> Result<()> {
        if !name.starts_with("Stopped") {
            self.ensure_active()?;
        }
        self.ui.stage(name);
        self.state["stage"] = json!(name);
        self.state["status"] = json!(match name {
            value if value.starts_with("Checked") => "ready",
            value if value.starts_with("Pull request") => "published",
            value if value.starts_with("Stopped") => {
                if self.stored.is_cancelled()? {
                    "cancelled"
                } else {
                    "stopped"
                }
            }
            _ => "active",
        });
        self.state["usage"] = json!({
            "total_tokens": self.usage.total_tokens(),
            "complete": self.usage.complete(self.state["limits"]["agents"].as_u64().unwrap_or(1) as usize),
            "calls": self.usage.records(),
            "max_tokens": self.usage.maximum(),
        });
        self.persist()
    }

    fn finish(&self) -> Result<()> {
        if self.output == OutputMode::Json {
            println!("{}", json_success_document("code", &self.state)?);
        } else {
            let markdown = report_markdown(&self.state, false);
            print_markdown(&markdown, self.theme)?;
            self.ui.note(&format!(
                "Run {} · Evidence: {}",
                self.stored.id(),
                self.scratch.join("run.html").display()
            ));
        }
        Ok(())
    }
}

pub fn deliver(mut config: Config) -> Result<Value> {
    validate_config(&config)?;
    let directory = config.directory.canonicalize()?;
    config.directory.clone_from(&directory);
    let orchestrator_harness = config.orchestrator_harness.unwrap_or(config.harness);
    let planning_model = config.orchestrator_model.as_deref().or_else(|| {
        if orchestrator_harness == config.harness {
            config
                .model_choices
                .first()
                .map(String::as_str)
                .or(config.model.as_deref())
        } else {
            None
        }
    });
    agent::executable(config.harness)?;
    if agent::which("git").is_none() || (config.repo.is_some() && agent::which("gh").is_none()) {
        bail!("install Git, and GitHub CLI for PR delivery");
    }

    let mut origin = None;
    let mut base = config.base.clone();
    if let Some(repo) = config.repo.as_deref() {
        github::validate_repository(repo)?;
        let remote = git(&directory, &["remote", "get-url", "origin"], None)?;
        if repository_from_remote(&remote).is_none_or(|value| !value.eq_ignore_ascii_case(repo)) {
            bail!("origin must match the explicit GitHub repository");
        }
        let info = github::api(&format!("repos/{repo}"), None, "GET", false)?
            .ok_or_else(|| anyhow!("GitHub returned no repository"))?;
        if info["full_name"]
            .as_str()
            .is_none_or(|value| !value.eq_ignore_ascii_case(repo))
            || info["permissions"]["push"].as_bool() != Some(true)
        {
            bail!("the selected GitHub login needs push access to this repository");
        }
        base.get_or_insert_with(|| info["default_branch"].as_str().unwrap_or("main").to_owned());
        git(
            &directory,
            &[
                "check-ref-format",
                "--branch",
                base.as_deref().unwrap_or("main"),
            ],
            None,
        )?;
        origin = Some(remote);
    } else if base.is_some() {
        bail!("--base requires --pr and --repo");
    }
    config.base.clone_from(&base);
    let network_auth = origin
        .as_deref()
        .map(git_network_auth)
        .transpose()?
        .flatten();

    let branch = format!("rady/{}", random_hex(6)?);
    let stored = RunStore::open()?.create()?;
    let scratch = stored.path().to_path_buf();
    let workspace = scratch.join("worktree");
    stored.write_json("config.json", &config)?;
    let usage = Usage::new(config.max_tokens)?;
    let state = json!({
        "schema": 1,
        "id": stored.id(),
        "created_unix_ms": u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()).unwrap_or(u64::MAX),
        "status": "created",
        "task": config.task,
        "repo": config.repo,
        "base": base,
        "branch": branch,
        "source_directory": directory,
        "resumed_from": config.resumed_from,
        "harness": config.harness.as_str(),
        "model": config.model,
        "orchestrator_harness": orchestrator_harness.as_str(),
        "orchestrator_model": config.orchestrator_model,
        "planning_model": planning_model,
        "model_choices": config.model_choices,
        "plan_source": if config.resumed_from.is_some() && config.plan.is_some() {
            "retained"
        } else if config.plan.is_some() {
            "specification"
        } else {
            "orchestrator"
        },
        "agent_runs": [],
        "checkpoints": [],
        "acceptance_checks": config.acceptance_checks,
        "workspace": workspace,
        "limits": {
            "agents": config.agents, "attempts": config.attempts,
            "max_files": config.max_files, "max_lines": config.max_lines,
            "timeout_seconds": config.timeout.as_secs_f64(), "max_tokens": config.max_tokens,
        },
        "gates": {},
    });
    let mut run = Run {
        scratch,
        stored,
        state,
        usage,
        ui: Ui::indeterminate(config.theme, config.output),
        theme: config.theme,
        output: config.output,
    };
    run.persist()?;
    run.ui
        .title("Rady", "A checked path from request to change");
    run.ui.note(&format!(
        "Run {} · cancel with `rady cancel {}`",
        run.stored.id(),
        run.stored.id()
    ));
    let result = run_delivery(
        &config,
        &directory,
        &workspace,
        &branch,
        base.as_deref(),
        origin.as_deref(),
        network_auth.as_ref(),
        orchestrator_harness,
        planning_model,
        &mut run,
    );
    match result {
        Ok(()) => {
            run.finish()?;
            Ok(run.state)
        }
        Err(error) => {
            run.state["error"] = json!(format!("{error:#}"));
            let cancel_file = run.scratch.join("cancelled");
            let stage = if run.stored.is_cancelled()? {
                "Stopped, workspace retained"
            } else {
                match retain_patch(&mut run, &workspace, Some(&cancel_file)) {
                    Ok(()) => "Stopped, work retained",
                    Err(retention_error) => {
                        run.state["retention_error"] = json!(retention_error.to_string());
                        "Stopped, workspace retained"
                    }
                }
            };
            run.stage(stage)?;
            Err(anyhow!(
                "{error:#}\nWorkspace and gate evidence: {}",
                run.scratch.display()
            ))
        }
    }
}

pub fn list_runs(theme: Theme, output: OutputMode) -> Result<()> {
    let mut runs = RunStore::open()?
        .list()?
        .into_iter()
        .map(|run| {
            match run.read_json::<Value>("run.json").and_then(|state| {
                validate_run_state(run.id(), &state)?;
                observed_run_state(&run, state)
            }) {
                Ok(state) => json!({
                    "id": run.id(),
                    "status": state["status"],
                    "stage": state["stage"],
                    "task": state["task"],
                    "created_unix_ms": state["created_unix_ms"],
                    "cancel_requested": state["cancel_requested"],
                    "path": run.path(),
                }),
                Err(error) => json!({
                    "id": run.id(), "status": "unreadable", "error": error.to_string(),
                    "path": run.path(),
                }),
            }
        })
        .collect::<Vec<_>>();
    runs.sort_by(|left, right| {
        right["created_unix_ms"]
            .as_u64()
            .cmp(&left["created_unix_ms"].as_u64())
    });
    if output == OutputMode::Json {
        println!("{}", serde_json::to_string_pretty(&json!({"runs": runs}))?);
        return Ok(());
    }
    let mut markdown = String::from("## Retained runs\n\n");
    if runs.is_empty() {
        markdown.push_str("No retained runs yet.\n");
    }
    for run in &runs {
        let pending = cancellation_pending(run);
        markdown.push_str(&format!(
            "- **{}** — {} · {}\n",
            markdown_text(run["id"].as_str().unwrap_or("unknown")),
            if pending {
                "stopping".to_owned()
            } else {
                markdown_text(run["status"].as_str().unwrap_or("unknown"))
            },
            if pending {
                "Cancellation requested — stopping at the current safe boundary".to_owned()
            } else {
                markdown_text(
                    run["stage"]
                        .as_str()
                        .or_else(|| run["error"].as_str())
                        .unwrap_or("No stage recorded"),
                )
            }
        ));
    }
    print_markdown(&markdown, theme)
}

pub fn inspect_run(id: &str, theme: Theme, output: OutputMode) -> Result<()> {
    let (stored, state) = load_run(id)?;
    let state = observed_run_state(&stored, state)?;
    if output == OutputMode::Json {
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else {
        let mut markdown = report_markdown(&state, false);
        markdown.push_str(&format!(
            "\n## Evidence\n\n- Run: {}\n- Files: {}\n",
            markdown_text(id),
            markdown_text(&stored.path().to_string_lossy())
        ));
        print_markdown(&markdown, theme)?;
    }
    Ok(())
}

pub fn cancel_run(id: &str, theme: Theme, output: OutputMode) -> Result<()> {
    let (stored, state) = load_run(id)?;
    match state["status"].as_str() {
        Some("created" | "active") => stored.cancel()?,
        Some("cancelled") => {}
        Some(status) => bail!("run {id} is already {status}"),
        None => bail!("run {id} has no valid status"),
    }
    if output == OutputMode::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"id": id, "cancel_requested": true}))?
        );
    } else {
        print_markdown(
            &format!(
                "## Cancellation requested\n\nRun `{}` will stop at the active boundary.\n",
                markdown_text(id)
            ),
            theme,
        )?;
    }
    Ok(())
}

pub fn resume_run(id: &str, theme: Theme, output: OutputMode) -> Result<()> {
    let (stored, state) = load_run(id)?;
    if !matches!(state["status"].as_str(), Some("stopped" | "cancelled")) {
        bail!("only a stopped or cancelled run can be resumed");
    }
    if state["url"].is_string() {
        bail!("a published run cannot be resumed");
    }
    if state["publication"].is_object() {
        bail!("publication may have started; inspect the retained branch before retrying");
    }
    let mut config: Config = stored.read_json("config.json")?;
    validate_config(&config)?;
    if config.plan.is_none() && state["plan"].is_object() {
        let plan: Plan = serde_json::from_value(state["plan"].clone())
            .map_err(|_| anyhow!("retained run has an invalid acceptance plan"))?;
        plan.validate()?;
        config.plan = Some(plan);
    }
    let patch = stored.path().join("changes.patch");
    match fs::symlink_metadata(&patch) {
        Ok(_) => {
            validate_patch(&patch, &state)?;
            config.seed_patch = Some(patch);
            config.expected_start = Some(
                state["start"]
                    .as_str()
                    .ok_or_else(|| anyhow!("retained run has no source revision"))?
                    .to_owned(),
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            config.seed_patch = None;
            config.expected_start = state["start"].as_str().map(str::to_owned);
        }
        Err(error) => return Err(error.into()),
    }
    if let Some(maximum) = config.max_tokens {
        config.max_tokens = Some(remaining_budget(maximum, &state["usage"])?);
    }
    config.resumed_from = Some(id.to_owned());
    config.theme = theme;
    config.output = output;
    deliver(config)?;
    Ok(())
}

pub fn apply_run(id: &str, directory: &Path, theme: Theme, output: OutputMode) -> Result<()> {
    let (stored, mut state) = load_run(id)?;
    if state["status"].as_str() != Some("ready") {
        bail!("only a checked, unpublished run can be applied");
    }
    let patch = stored.path().join("changes.patch");
    validate_patch(&patch, &state)?;
    let start = state["start"]
        .as_str()
        .ok_or_else(|| anyhow!("retained run has no source revision"))?;
    let names: Vec<String> = serde_json::from_value(state["changed_files"].clone())
        .map_err(|_| anyhow!("retained run has invalid changed files"))?;
    let expected: BTreeMap<String, Fingerprint> =
        serde_json::from_value(state["candidate"].clone())
            .map_err(|_| anyhow!("retained run has invalid file fingerprints"))?;
    if names.is_empty()
        || names.iter().any(|name| !quality::relative_path(name))
        || expected.keys().ne(names.iter())
    {
        bail!("retained run has inconsistent changed-file evidence");
    }
    let requested = directory.canonicalize()?;
    let root =
        PathBuf::from(git(&requested, &["rev-parse", "--show-toplevel"], None)?).canonicalize()?;
    if git(&root, &["rev-parse", "HEAD"], None)? != start {
        bail!("target revision differs from the verified run");
    }
    if !git(
        &root,
        &["status", "--porcelain", "--untracked-files=all"],
        None,
    )?
    .is_empty()
    {
        bail!("target directory must be clean before applying a retained run");
    }
    let patch_text = patch
        .to_str()
        .ok_or_else(|| anyhow!("retained patch path is not valid UTF-8"))?;
    git(
        &root,
        &["apply", "--check", "--binary", "--", patch_text],
        None,
    )?;
    git(&root, &["apply", "--binary", "--", patch_text], None)?;
    let verified = changed_files(&root, start, None)
        .and_then(|actual| Ok(actual == names && snapshot(&root, &names)? == expected));
    if !matches!(verified, Ok(true)) {
        let rollback = git(
            &root,
            &["apply", "--reverse", "--binary", "--", patch_text],
            None,
        );
        let clean = git(
            &root,
            &["status", "--porcelain", "--untracked-files=all"],
            None,
        )
        .is_ok_and(|status| status.is_empty());
        if rollback.is_ok() && clean {
            bail!("applied files failed integrity verification; the target was rolled back");
        }
        bail!(
            "applied files failed integrity verification and automatic rollback failed; recovery patch: {}",
            patch.display()
        );
    }
    state["status"] = json!("applied");
    state["stage"] = json!("Applied to local checkout");
    state["applied_to"] = json!(root);
    stored.write_json("run.json", &state)?;
    write_report(
        &stored.path().join("run.html"),
        "Applied to local checkout",
        &report_markdown(&state, true),
        ReportState::Complete,
        theme,
    )?;
    if output == OutputMode::Json {
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else {
        print_markdown(&report_markdown(&state, false), theme)?;
    }
    Ok(())
}

fn load_run(id: &str) -> Result<(StoredRun, Value)> {
    let stored = RunStore::open()?.load(id)?;
    let state: Value = stored.read_json("run.json")?;
    validate_run_state(id, &state)?;
    Ok((stored, state))
}

fn observed_run_state(stored: &StoredRun, mut state: Value) -> Result<Value> {
    state["cancel_requested"] = json!(stored.is_cancelled()?);
    Ok(state)
}

fn cancellation_pending(state: &Value) -> bool {
    state["cancel_requested"].as_bool() == Some(true)
        && matches!(state["status"].as_str(), Some("created" | "active"))
}

fn validate_run_state(id: &str, state: &Value) -> Result<()> {
    if state["schema"].as_u64() != Some(1)
        || state["id"].as_str() != Some(id)
        || !matches!(
            state["status"].as_str(),
            Some(
                "created" | "active" | "ready" | "stopped" | "cancelled" | "published" | "applied"
            )
        )
    {
        bail!("retained run metadata is invalid");
    }
    Ok(())
}

fn remaining_budget(maximum: u64, usage: &Value) -> Result<u64> {
    if usage["complete"].as_bool() != Some(true) {
        bail!("token usage is incomplete, so this budgeted run cannot resume");
    }
    maximum
        .checked_sub(
            usage["total_tokens"]
                .as_u64()
                .ok_or_else(|| anyhow!("retained run has invalid token usage"))?,
        )
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| anyhow!("token budget is exhausted"))
}

fn validate_patch(path: &Path, state: &Value) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4_000_000 {
        bail!("retained patch must be a regular file no larger than 4 MB");
    }
    let expected = state["patch_sha256"]
        .as_str()
        .ok_or_else(|| anyhow!("retained run has no patch fingerprint"))?;
    if hash_file(path)? != expected {
        bail!("retained patch fingerprint does not match");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_delivery(
    config: &Config,
    directory: &Path,
    workspace: &Path,
    branch: &str,
    base: Option<&str>,
    origin: Option<&str>,
    network_auth: Option<&GitNetworkAuth>,
    orchestrator_harness: Harness,
    planning_model: Option<&str>,
    run: &mut Run,
) -> Result<()> {
    let cancel_file = run.scratch.join("cancelled");
    run.stage("Preparing an isolated workspace")?;
    if config.repo.is_some() && config.expected_start.is_none() {
        git_network(
            directory,
            &["fetch", "--no-tags", "--", "origin", base.unwrap_or("main")],
            Some(&cancel_file),
            network_auth,
        )?;
    }
    git(
        directory,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            workspace.to_string_lossy().as_ref(),
            config
                .expected_start
                .as_deref()
                .unwrap_or(if config.repo.is_some() {
                    "FETCH_HEAD"
                } else {
                    "HEAD"
                }),
        ],
        Some(&cancel_file),
    )?;
    let start = git(workspace, &["rev-parse", "HEAD"], Some(&cancel_file))?;
    let mut expected_head = start.clone();
    if config
        .expected_start
        .as_deref()
        .is_some_and(|value| value != start)
    {
        bail!("the source revision changed; start a fresh run on the new revision");
    }
    run.state["start"] = json!(start);
    run.stage("Workspace isolated")?;
    let original = snapshot(
        workspace,
        &changed_files(workspace, &start, Some(&cancel_file))?,
    )?;
    let repository_context = RepositoryContext::load(workspace, &config.mcp_servers)?;
    let pinned_context = snapshot(workspace, repository_context.files())?;
    run.state["context"] = json!({
        "files": repository_context.files(),
        "mcp_servers": config.mcp_servers,
    });

    let plan = if let Some(plan) = &config.plan {
        run.stage(if config.resumed_from.is_some() {
            "Using retained acceptance criteria"
        } else {
            "Using supplied acceptance criteria"
        })?;
        plan.clone()
    } else {
        run.stage("Shaping acceptance criteria")?;
        quality::plan(
            &config.task,
            workspace,
            orchestrator_harness,
            planning_model,
            config.timeout,
            if config.model.is_none() {
                &config.model_choices
            } else {
                &[]
            },
            repository_context.guidance(),
            Some(&mut run.usage),
            Some(&cancel_file),
        )?
    };
    plan.validate()?;
    unchanged(
        workspace,
        &start,
        branch,
        &expected_head,
        &original,
        &BTreeMap::new(),
        &pinned_context,
        &config.acceptance_checks,
        Some(&cancel_file),
    )?;
    run.stored
        .write_json("spec.json", &json!({"request": config.task, "plan": plan}))?;
    run.state["plan"] = serde_json::to_value(&plan)?;
    run.state["gates"]["specification"] = json!("pass");
    if plan.performance_required && config.benchmarks.is_empty() {
        bail!("benchmark gate: performance requirements need an explicit --benchmark command");
    }
    let frozen = verification_files(workspace, &config.acceptance_checks)?;
    let baseline_acceptance = acceptance(
        &config.acceptance_checks,
        workspace,
        config.timeout,
        &run.stored,
        "before",
        Some(&cancel_file),
    )?;
    if verification_files(workspace, &config.acceptance_checks)? != frozen {
        bail!("acceptance verification files changed during baseline checks");
    }
    run.state["acceptance_before"] = json!(baseline_acceptance);
    run.state["gates"]["independent_acceptance"] = if config.acceptance_checks.is_empty() {
        json!("not supplied: coverage relies on model review; PR publication blocked")
    } else {
        json!("pending")
    };
    let benchmark_commands = parse_commands(&config.benchmarks)?;
    let before = if benchmark_commands.is_empty() {
        Vec::new()
    } else {
        benchmark::measure(
            &benchmark_commands,
            workspace,
            config.timeout,
            &run.scratch,
            "before",
            config.benchmark_runs,
            config.benchmark_warmups,
            config.benchmark_metric.as_deref(),
            |command, directory, timeout| {
                run_check(command, directory, timeout, Some(&cancel_file))
            },
        )?
    };
    run.state["benchmarks_before"] = json!(before);
    unchanged(
        workspace,
        &start,
        branch,
        &expected_head,
        &original,
        &frozen,
        &pinned_context,
        &config.acceptance_checks,
        Some(&cancel_file),
    )?;

    let commands = parse_commands(&config.checks)?;
    let mut feedback = if config.resumed_from.is_some() {
        "\nContinue from the retained change. Inspect it before editing and repair only what the acceptance evidence requires.".to_owned()
    } else {
        String::new()
    };
    let mut rejected: Option<BTreeMap<String, Fingerprint>> = None;
    let mut candidate = original.clone();
    let mut names = Vec::new();
    let mut diff = String::new();
    let mut report: Option<ReviewReport> = None;
    let mut verified = Vec::new();
    let mut after = Vec::<Measurement>::new();
    let mut model_index = plan.model_index.unwrap_or(0);
    if let Some(seed_patch) = config.seed_patch.as_deref() {
        let metadata = fs::symlink_metadata(seed_patch)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4_000_000 {
            bail!("retained patch must be a regular file no larger than 4 MB");
        }
        run.stage("Restoring the retained change")?;
        let patch = seed_patch
            .to_str()
            .ok_or_else(|| anyhow!("retained patch path is not valid UTF-8"))?;
        git(
            workspace,
            &["apply", "--check", "--binary", "--", patch],
            Some(&cancel_file),
        )?;
        git(
            workspace,
            &["apply", "--binary", "--", patch],
            Some(&cancel_file),
        )?;
        (names, diff, candidate) = evidence(
            workspace,
            &start,
            config.max_files,
            config.max_lines,
            Some(&cancel_file),
        )?;
        quality::enforce_scope(&names, &plan.scope)?;
        if verification_files(workspace, &config.acceptance_checks)? != frozen {
            bail!("retained patch changed frozen verification files");
        }
        retain_candidate(
            run,
            &names,
            &diff,
            &candidate,
            workspace,
            &start,
            Some(&cancel_file),
        )?;
    }

    for attempt in 1..=config.attempts {
        let worker_model = config
            .model
            .as_deref()
            .or_else(|| config.model_choices.get(model_index).map(String::as_str));
        let reviewer_model = config
            .review_model
            .as_deref()
            .or(config.orchestrator_model.as_deref())
            .or(if orchestrator_harness == config.harness {
                worker_model
            } else {
                None
            });
        let jobs: Vec<Option<(usize, quality::Task)>> =
            if attempt == 1 && config.resumed_from.is_none() && !plan.tasks.is_empty() {
                plan.tasks.iter().cloned().enumerate().map(Some).collect()
            } else {
                vec![None]
            };
        let mut worker_runs = Vec::new();
        for job in jobs {
            let previous = snapshot(
                workspace,
                &changed_files(workspace, &start, Some(&cancel_file))?,
            )?;
            let (prompt, selected_model, task_index) = if let Some((index, task)) = job {
                let selected = config.model.as_deref().or_else(|| {
                    config
                        .model_choices
                        .get(task.model_index.unwrap_or(model_index))
                        .map(String::as_str)
                });
                let specification = json!({
                    "description": task.description,
                    "scope": task.scope,
                    "acceptance": task.acceptance.iter().map(|index| &plan.acceptance[*index]).collect::<Vec<_>>(),
                    "limitations": plan.limitations,
                    "dependencies": task.depends_on.iter().map(|index| json!({"description": plan.tasks[*index].description, "scope": plan.tasks[*index].scope})).collect::<Vec<_>>()
                });
                (
                    format!(
                        "{}\n\nSubtask {}/{}\nImplement only this task and preserve earlier work.\n{}",
                        config.task,
                        index + 1,
                        plan.tasks.len(),
                        serde_json::to_string(&specification)?
                    ),
                    selected,
                    Some(index),
                )
            } else {
                (
                    format!(
                        "{}\n\nAcceptance plan:\n{}{}",
                        config.task,
                        serde_json::to_string(&plan)?,
                        feedback
                    ),
                    worker_model,
                    None,
                )
            };
            let prompt = if frozen.is_empty() {
                prompt
            } else {
                format!(
                    "{prompt}\nDo not edit these operator-owned verification files: {}",
                    serde_json::to_string(&frozen.keys().collect::<Vec<_>>())?
                )
            };
            let stage = if let Some(index) = task_index {
                format!("Implementing task {}/{}", index + 1, plan.tasks.len())
            } else {
                format!("Implementing attempt {attempt}/{}", config.attempts)
            };
            run.stage(&stage)?;
            let started = Instant::now();
            let output = run_worker(
                &prompt,
                workspace,
                selected_model,
                config.agents,
                config.harness,
                config.timeout,
                &mut run.usage,
                repository_context.guidance(),
                repository_context.mcp(),
                Some(&cancel_file),
            )?;
            let log_name = format!("worker-{attempt}-{}.log", worker_runs.len() + 1);
            run.stored
                .write_text(&log_name, &format!("{}{}", output.stdout, output.stderr))?;
            let worker_message = agent::worker_message(&output, config.harness)?;
            let worker_run = json!({
                "role": "worker", "harness": config.harness.as_str(), "model": selected_model,
                "task_index": task_index, "outcome": if output.code == 0 { "completed" } else { "failed" },
                "elapsed_seconds": started.elapsed().as_secs_f64(), "message": tail(&worker_message, 2000),
                "log": run.scratch.join(&log_name),
            });
            run.state["agent_runs"]
                .as_array_mut()
                .ok_or_else(|| anyhow!("invalid run state"))?
                .push(worker_run.clone());
            worker_runs.push(worker_run);
            if output.code != 0 {
                bail!("the agent stopped before completing the task");
            }
            (names, diff, candidate) = evidence(
                workspace,
                &start,
                config.max_files,
                config.max_lines,
                Some(&cancel_file),
            )?;
            retain_candidate(
                run,
                &names,
                &diff,
                &candidate,
                workspace,
                &start,
                Some(&cancel_file),
            )?;
            quality::enforce_scope(&names, &plan.scope)?;
            if let Some(index) = task_index {
                let touched = changed_between(&previous, &candidate);
                quality::enforce_scope(&touched, &plan.tasks[index].scope)?;
            }
            if verification_files(workspace, &config.acceptance_checks)? != frozen {
                bail!("acceptance gate: worker changed frozen verification files");
            }
            unchanged(
                workspace,
                &start,
                branch,
                &expected_head,
                &candidate,
                &frozen,
                &pinned_context,
                &config.acceptance_checks,
                Some(&cancel_file),
            )?;
            if config.repo.is_some() && !config.ghost {
                let label = task_index.map_or_else(
                    || format!("repair attempt {attempt}"),
                    |index| plan.tasks[index].description.clone(),
                );
                if let Some(commit) = checkpoint(workspace, &names, &label, Some(&cancel_file))? {
                    expected_head.clone_from(&commit);
                    run.state["checkpoints"]
                        .as_array_mut()
                        .ok_or_else(|| anyhow!("invalid run state"))?
                        .push(json!({"commit": commit, "task_index": task_index, "label": label}));
                    run.persist()?;
                }
            }
        }
        if rejected.as_ref() == Some(&candidate) {
            run.state["gates"]["progress"] = json!("fail");
            bail!("efficiency gate: repair made no changes; repeated validation was skipped");
        }
        run.state["gates"]["scope"] = json!("pass");
        run.state["gates"]["scope_limits"] = json!("pass");
        run.state["gates"]["credential_scan"] = json!("pass");
        run.stage(&format!(
            "Running checks · attempt {attempt}/{}",
            config.attempts
        ))?;
        let mut failures = Vec::new();
        let mut check_results = Vec::with_capacity(commands.len());
        for (index, command) in commands.iter().enumerate() {
            run.ensure_active()?;
            let (status, output) =
                run_check(command, workspace, config.timeout, Some(&cancel_file))?;
            let log_name = format!("check-{attempt}-{}.log", index + 1);
            run.stored.write_text(&log_name, &output)?;
            check_results.push(json!({
                "command": join_command(command), "exit_code": status,
                "output_tail": tail(&output, (6000 / commands.len()).clamp(1, 1500)),
                "log": run.scratch.join(log_name),
            }));
            if status != 0 {
                failures.push(format!(
                    "{}\n{}",
                    join_command(command),
                    tail(&output, 6000)
                ));
            }
        }
        run.state["checks"] = json!(check_results);
        run.state["gates"]["checks"] = json!(if failures.is_empty() { "pass" } else { "fail" });
        unchanged(
            workspace,
            &start,
            branch,
            &expected_head,
            &candidate,
            &frozen,
            &pinned_context,
            &config.acceptance_checks,
            Some(&cancel_file),
        )?;
        verified = acceptance(
            &config.acceptance_checks,
            workspace,
            config.timeout,
            &run.stored,
            &attempt.to_string(),
            Some(&cancel_file),
        )?;
        run.state["acceptance_after"] = json!(verified);
        if !config.acceptance_checks.is_empty() {
            run.state["gates"]["independent_acceptance"] =
                json!(if verified.iter().all(|value| value["status"] == "pass") {
                    "pass"
                } else {
                    "fail"
                });
        }
        failures.extend(
            verified
                .iter()
                .filter(|value| value["status"] == "fail")
                .map(|value| {
                    format!(
                        "Acceptance criterion {} failed: {}\n{}",
                        value["criterion"].as_u64().unwrap_or(0) + 1,
                        value["command"].as_str().unwrap_or_default(),
                        value["output_tail"].as_str().unwrap_or_default()
                    )
                }),
        );
        if !benchmark_commands.is_empty() && failures.is_empty() {
            after = benchmark::measure(
                &benchmark_commands,
                workspace,
                config.timeout,
                &run.scratch,
                &attempt.to_string(),
                config.benchmark_runs,
                config.benchmark_warmups,
                config.benchmark_metric.as_deref(),
                |command, directory, timeout| {
                    run_check(command, directory, timeout, Some(&cancel_file))
                },
            )?;
            failures.extend(benchmark::compare(
                &before,
                &after,
                config.max_regression,
                config.max_benchmark_noise,
            )?);
        }
        run.state["benchmarks_after"] = json!(after);
        if !failures.is_empty() {
            rejected = Some(candidate.clone());
            feedback = format!(
                "\nFix these failures without weakening checks:\n{}",
                failures.join("\n").chars().take(12_000).collect::<String>()
            );
            continue;
        }
        run.stage("Reviewing the checked evidence")?;
        let evidence = json!({
            "checks": check_results,
            "acceptance_before": baseline_acceptance,
            "acceptance_after": verified,
            "benchmarks_before": before,
            "benchmarks_after": after,
        });
        let reviewed = quality::review(
            &config.task,
            &plan,
            &diff,
            &names,
            &evidence,
            workspace,
            orchestrator_harness,
            reviewer_model,
            config.timeout,
            repository_context.guidance(),
            Some(&mut run.usage),
            Some(&cancel_file),
        )?;
        let blockers = quality::blockers(&reviewed)?;
        run.state["review"] = serde_json::to_value(&reviewed)?;
        for (name, gate) in &reviewed.gates {
            run.state["gates"][name] = serde_json::to_value(gate)?;
        }
        report = Some(reviewed);
        if blockers.is_empty() {
            break;
        }
        rejected = Some(candidate.clone());
        if config.model.is_none() && !config.model_choices.is_empty() {
            model_index = (model_index + 1).min(config.model_choices.len() - 1);
        }
        feedback = format!(
            "\nAddress these independent review findings:\n{}",
            blockers.join("\n").chars().take(12_000).collect::<String>()
        );
    }
    let report = report.ok_or_else(|| {
        anyhow!("checks or review still need attention after the configured attempts")
    })?;
    unchanged(
        workspace,
        &start,
        branch,
        &expected_head,
        &candidate,
        &frozen,
        &pinned_context,
        &config.acceptance_checks,
        Some(&cancel_file),
    )?;
    run.state["gates"]["unchanged_after_validation"] = json!("pass");
    if config.repo.is_none() {
        run.stage("Checked change ready")?;
        run.ui.success("The checked workspace is ready");
        return Ok(());
    }
    run.stage("Publishing the checked change")?;
    if git(
        directory,
        &["remote", "get-url", "origin"],
        Some(&cancel_file),
    )? != origin.unwrap_or_default()
    {
        bail!("origin changed during the task");
    }
    let mut add = vec!["--literal-pathspecs", "add", "--"];
    add.extend(names.iter().map(String::as_str));
    git(workspace, &add, Some(&cancel_file))?;
    git(
        workspace,
        &["diff", "--cached", "--check"],
        Some(&cancel_file),
    )?;
    git(
        workspace,
        &[
            "diff",
            "--exit-code",
            "--no-ext-diff",
            "--no-textconv",
            "--",
        ],
        Some(&cancel_file),
    )?;
    unchanged(
        workspace,
        &start,
        branch,
        &expected_head,
        &candidate,
        &frozen,
        &pinned_context,
        &config.acceptance_checks,
        Some(&cancel_file),
    )?;
    let title = config
        .task
        .lines()
        .next()
        .unwrap_or("Implement the requested specification")
        .trim_start_matches(['#', ' '])
        .chars()
        .take(72)
        .collect::<String>();
    run.ensure_active()?;
    if let Some(expected_start) = config.expected_start.as_deref() {
        let base = base.ok_or_else(|| anyhow!("a pinned publication requires a base branch"))?;
        let origin = origin.ok_or_else(|| anyhow!("a pinned publication requires an origin"))?;
        require_remote_base(
            workspace,
            origin,
            base,
            expected_start,
            Some(&cancel_file),
            network_auth,
        )?;
    }
    let commit = if config.ghost {
        git(workspace, &["commit", "-m", &title], Some(&cancel_file))?;
        let commit = git(workspace, &["rev-parse", "HEAD"], Some(&cancel_file))?;
        if git(workspace, &["rev-parse", "HEAD^"], Some(&cancel_file))? != start {
            bail!("ghost delivery must contain exactly one verified commit");
        }
        commit
    } else {
        let commit = git(workspace, &["rev-parse", "HEAD"], Some(&cancel_file))?;
        if commit == start || commit != expected_head {
            bail!("progressive delivery has no verified checkpoint");
        }
        git(
            workspace,
            &["merge-base", "--is-ancestor", &start, &commit],
            Some(&cancel_file),
        )?;
        commit
    };
    if git(
        workspace,
        &["symbolic-ref", "--short", "HEAD"],
        Some(&cancel_file),
    )? != branch
        || !git(workspace, &["status", "--porcelain"], Some(&cancel_file))?.is_empty()
    {
        bail!("workspace changed during commit; publishing is blocked");
    }
    let repo = config.repo.as_deref().unwrap_or_default();
    run.ensure_active()?;
    run.state["publication"] = json!({
        "status": "branch push requested", "repository": repo,
        "branch": branch, "commit": commit,
    });
    run.persist()?;
    git_network(
        workspace,
        &[
            "push",
            "--porcelain",
            "origin",
            &format!("{commit}:refs/heads/{branch}"),
        ],
        Some(&cancel_file),
        network_auth,
    )?;
    run.state["publication"]["status"] = json!("branch pushed");
    run.persist()?;
    let body = pull_request_body(config, &plan, &report, &verified, &after);
    run.ensure_active()?;
    run.state["publication"]["status"] = json!("pull request requested");
    run.persist()?;
    let response = github::api_cancellable(
        &format!("repos/{repo}/pulls"),
        Some(
            &json!({"title": title, "head": branch, "base": base.unwrap_or("main"), "body": body}),
        ),
        "POST",
        false,
        &cancel_file,
    )?
    .ok_or_else(|| anyhow!("GitHub returned no pull request"))?;
    let url = response["html_url"].as_str().unwrap_or_default();
    let expected = Regex::new(&format!(
        r"^https://github\.com/{}/pull/[1-9][0-9]*$",
        regex::escape(repo)
    ))?;
    if !expected.is_match(url) {
        bail!("GitHub returned an unexpected PR response; inspect the branch before retrying");
    }
    run.state["url"] = json!(url);
    run.state["publication"]["status"] = json!("pull request opened");
    run.state["publication"]["url"] = json!(url);
    run.persist()?;
    run.stage("Pull request opened")?;
    run.ui.success(url);
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

fn git(directory: &Path, arguments: &[&str], cancel_file: Option<&Path>) -> Result<String> {
    let binary = agent::which("git").ok_or_else(|| anyhow!("install Git"))?;
    let output = agent::execute(
        binary.as_os_str(),
        arguments,
        directory,
        b"",
        Duration::from_secs(120),
        &BTreeMap::from([("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())]),
        false,
        cancel_file,
    )?;
    if output.code != 0 {
        bail!(
            "Git {} failed; inspect the retained workspace before retrying",
            arguments.first().copied().unwrap_or("command")
        );
    }
    Ok(output.stdout.trim_end_matches('\n').to_owned())
}

fn checkpoint(
    workspace: &Path,
    names: &[String],
    label: &str,
    cancel_file: Option<&Path>,
) -> Result<Option<String>> {
    if git(workspace, &["status", "--porcelain"], cancel_file)?.is_empty() {
        return Ok(None);
    }
    let mut add = vec!["--literal-pathspecs", "add", "--"];
    add.extend(names.iter().map(String::as_str));
    git(workspace, &add, cancel_file)?;
    git(workspace, &["diff", "--cached", "--check"], cancel_file)?;
    let label = label
        .lines()
        .next()
        .unwrap_or("checkpoint")
        .trim_start_matches(['#', ' '])
        .chars()
        .take(64)
        .collect::<String>();
    let message = format!(
        "rady: {}",
        if label.is_empty() {
            "checkpoint"
        } else {
            &label
        }
    );
    git(workspace, &["commit", "-m", &message], cancel_file)?;
    if !git(workspace, &["status", "--porcelain"], cancel_file)?.is_empty() {
        bail!("workspace changed during checkpoint; publishing is blocked");
    }
    Ok(Some(git(workspace, &["rev-parse", "HEAD"], cancel_file)?))
}

fn git_network(
    directory: &Path,
    arguments: &[&str],
    cancel_file: Option<&Path>,
    auth: Option<&GitNetworkAuth>,
) -> Result<String> {
    let binary = agent::which("git").ok_or_else(|| anyhow!("install Git"))?;
    let mut environment = BTreeMap::from([("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())]);
    if let Some(auth) = auth {
        environment.extend(auth.environment.clone());
    }
    let output = agent::execute(
        binary.as_os_str(),
        arguments,
        directory,
        b"",
        Duration::from_secs(120),
        &environment,
        false,
        cancel_file,
    )?;
    if output.code != 0 {
        bail!(
            "Git {} failed; inspect the retained workspace before retrying",
            arguments.first().copied().unwrap_or("command")
        );
    }
    Ok(output.stdout.trim_end_matches('\n').to_owned())
}

fn require_remote_base(
    directory: &Path,
    remote: &str,
    base: &str,
    expected_start: &str,
    cancel_file: Option<&Path>,
    auth: Option<&GitNetworkAuth>,
) -> Result<()> {
    let reference = format!("refs/heads/{base}");
    let output = git_network(
        directory,
        &["ls-remote", "--exit-code", remote, &reference],
        cancel_file,
        auth,
    )?;
    if parse_remote_ref(&output, &reference)? != expected_start {
        bail!("the base branch advanced during the task; start a fresh run");
    }
    Ok(())
}

fn parse_remote_ref<'a>(output: &'a str, reference: &str) -> Result<&'a str> {
    let (object_id, received_reference) = output
        .split_once('\t')
        .ok_or_else(|| anyhow!("Git returned an invalid remote reference"))?;
    if output.matches('\t').count() != 1
        || received_reference != reference
        || !matches!(object_id.len(), 40 | 64)
        || !object_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("Git returned an invalid remote reference");
    }
    Ok(object_id)
}

fn git_network_auth(remote: &str) -> Result<Option<GitNetworkAuth>> {
    match env::var("RADY_PUSH_TOKEN") {
        Ok(token) => Ok(Some(GitNetworkAuth {
            environment: git_network_environment(remote, &token)?,
        })),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => bail!("RADY_PUSH_TOKEN must contain valid text"),
    }
}

fn git_network_environment(remote: &str, token: &str) -> Result<BTreeMap<String, String>> {
    if token.is_empty()
        || token.len() > MAX_PUSH_TOKEN_BYTES
        || !token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        bail!("RADY_PUSH_TOKEN must be 1 to {MAX_PUSH_TOKEN_BYTES} printable ASCII characters");
    }
    if remote
        .strip_prefix("https://github.com/")
        .is_none_or(str::is_empty)
    {
        bail!("RADY_PUSH_TOKEN requires an HTTPS github.com origin");
    }
    let credentials = format!("x-access-token:{token}");
    Ok(BTreeMap::from([
        ("GIT_CONFIG_COUNT".to_owned(), "1".to_owned()),
        (
            "GIT_CONFIG_KEY_0".to_owned(),
            "http.https://github.com/.extraheader".to_owned(),
        ),
        (
            "GIT_CONFIG_VALUE_0".to_owned(),
            format!("AUTHORIZATION: Basic {}", base64(credentials.as_bytes())),
        ),
    ]))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        encoded.push(ALPHABET[(chunk[0] >> 2) as usize] as char);
        encoded.push(
            ALPHABET[(((chunk[0] & 3) << 4) | (chunk.get(1).copied().unwrap_or(0) >> 4)) as usize]
                as char,
        );
        match chunk {
            [_, second, third] => {
                encoded.push(ALPHABET[(((second & 15) << 2) | (third >> 6)) as usize] as char);
                encoded.push(ALPHABET[(third & 63) as usize] as char);
            }
            [_, second] => {
                encoded.push(ALPHABET[((second & 15) << 2) as usize] as char);
                encoded.push('=');
            }
            [_] => encoded.push_str("=="),
            _ => unreachable!("chunks are never empty"),
        }
    }
    encoded
}

fn run_check(
    command: &[String],
    directory: &Path,
    timeout: Duration,
    cancel_file: Option<&Path>,
) -> Result<(i32, String)> {
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| anyhow!("check command is empty"))?;
    let output = agent::execute(
        program.as_ref(),
        arguments,
        directory,
        b"",
        timeout,
        &BTreeMap::new(),
        true,
        cancel_file,
    )?;
    let text = if output.stdout.len() > 64_000 {
        format!("[Earlier output omitted]\n{}", tail(&output.stdout, 64_000))
    } else {
        output.stdout
    };
    Ok((output.code, text))
}

fn changed_files(
    directory: &Path,
    reference: &str,
    cancel_file: Option<&Path>,
) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    for name in git(
        directory,
        &["diff", "--name-only", "-z", reference, "--"],
        cancel_file,
    )?
    .split('\0')
    {
        if !name.is_empty() {
            names.insert(name.to_owned());
        }
    }
    for name in git(
        directory,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        cancel_file,
    )?
    .split('\0')
    {
        if !name.is_empty() {
            names.insert(name.to_owned());
        }
    }
    let credential = Regex::new(CREDENTIAL_PATTERN)?;
    for name in &names {
        if !quality::relative_path(name) {
            bail!("refusing a path outside the task workspace");
        }
        let path = Path::new(name);
        let basename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let suffix = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if basename == ".env"
            || (basename.starts_with(".env.") && !matches!(suffix.as_str(), "example" | "sample"))
            || basename == "auth.json"
            || matches!(suffix.as_str(), "pem" | "p12" | "pfx")
        {
            bail!("potential credential file requires manual review: {name}");
        }
        scan_credentials(directory, path, name, &credential)?;
    }
    Ok(names.into_iter().collect())
}

const MAX_CREDENTIAL_SCAN_BYTES: usize = 1_000_000;
const CREDENTIAL_PATTERN: &str = r"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----|(?:github_pat_[A-Za-z0-9_]{20,255}|gh[pousr]_[A-Za-z0-9]{20,255}|(?:AKIA|ASIA)[0-9A-Z]{16}|(?:AIza[A-Za-z0-9_-]{35}|AQ\.[A-Za-z0-9_-]{20,255}|csk-[A-Za-z0-9_-]{20,255}|xai-[A-Za-z0-9_-]{20,255})|sk-(?:ant-[A-Za-z0-9_-]{20,255}|proj-[A-Za-z0-9_-]{20,255}|[A-Za-z0-9_-]{32,255}))";

fn scan_credentials(directory: &Path, path: &Path, name: &str, credential: &Regex) -> Result<()> {
    let Some(file) = open_scannable_file(directory, path, name)? else {
        return Ok(());
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() > MAX_CREDENTIAL_SCAN_BYTES as u64
        || metadata.nlink() > 1
    {
        bail!("credential scan requires a unique file no larger than 1 MB: {name}");
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
    file.take((MAX_CREDENTIAL_SCAN_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CREDENTIAL_SCAN_BYTES {
        bail!("credential scan file grew beyond 1 MB: {name}");
    }
    let content = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow!("credential scan requires text or an approved binary path: {name}"))?;
    if credential.is_match(content) {
        bail!("credential material requires manual review: {name}");
    }
    Ok(())
}

fn open_scannable_file(directory: &Path, path: &Path, name: &str) -> Result<Option<File>> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("refusing a path outside the task workspace");
    }
    let directory_flags = OFlag::O_RDONLY
        | OFlag::O_CLOEXEC
        | OFlag::O_DIRECTORY
        | OFlag::O_NOFOLLOW
        | OFlag::O_NONBLOCK;
    let mut parent = open(directory, directory_flags, Mode::empty())
        .map_err(|error| anyhow!("cannot safely open the task workspace: {error}"))?;
    let mut components = path.components().peekable();
    while let Some(Component::Normal(component)) = components.next() {
        let final_component = components.peek().is_none();
        let flags = if final_component {
            OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK
        } else {
            directory_flags
        };
        match openat(&parent, component, flags, Mode::empty()) {
            Ok(file) if final_component => return Ok(Some(file.into())),
            Ok(directory) => parent = directory,
            Err(Errno::ENOENT) => return Ok(None),
            Err(Errno::ELOOP) if final_component => {
                bail!("changed symlink requires manual review: {name}")
            }
            Err(error) => bail!("cannot safely scan changed file {name}: {error}"),
        }
    }
    bail!("refusing a path outside the task workspace")
}

fn snapshot(directory: &Path, names: &[String]) -> Result<BTreeMap<String, Fingerprint>> {
    names
        .iter()
        .map(|name| {
            let path = directory.join(name);
            let value = match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Fingerprint::Missing,
                Err(error) => return Err(error.into()),
                Ok(metadata) if metadata.file_type().is_symlink() => Fingerprint::Symlink {
                    mode: metadata.mode(),
                    target: fs::read_link(&path)?.to_string_lossy().into_owned(),
                },
                Ok(metadata) if metadata.is_file() => {
                    let file = open_scannable_file(directory, Path::new(name), name)?.ok_or_else(
                        || anyhow!("changed file disappeared while capturing evidence: {name}"),
                    )?;
                    let metadata = file.metadata()?;
                    if !metadata.is_file() {
                        bail!("unsupported changed file requires manual review: {name}");
                    }
                    Fingerprint::File {
                        mode: metadata.mode(),
                        sha256: hash_reader(file)?,
                    }
                }
                Ok(_) => bail!("unsupported changed file requires manual review: {name}"),
            };
            Ok((name.clone(), value))
        })
        .collect()
}

fn hash_file(path: &Path) -> Result<String> {
    hash_reader(File::open(path)?)
}

fn hash_reader(mut file: File) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

fn evidence(
    directory: &Path,
    start: &str,
    max_files: usize,
    max_lines: usize,
    cancel_file: Option<&Path>,
) -> Result<(Vec<String>, String, BTreeMap<String, Fingerprint>)> {
    let names = changed_files(directory, start, cancel_file)?;
    if names.is_empty() {
        bail!("no changes were produced");
    }
    if names.len() > max_files {
        bail!(
            "efficiency gate: {} changed files exceeds {max_files}",
            names.len()
        );
    }
    let mut diff = git(
        directory,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=3",
            start,
            "--",
        ],
        cancel_file,
    )?;
    for name in git(
        directory,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        cancel_file,
    )?
    .split('\0')
    .filter(|name| !name.is_empty())
    {
        let Some(file) = open_scannable_file(directory, Path::new(name), name)? else {
            bail!("changed file disappeared while capturing evidence: {name}");
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            bail!("unsupported changed file requires manual review: {name}");
        }
        if metadata.len() > 64_000 {
            bail!("review context exceeds 64000 bytes; split the task");
        }
        let mut content = String::new();
        file.take(64_001).read_to_string(&mut content)?;
        if content.len() > 64_000 {
            bail!("review context exceeds 64000 bytes; split the task");
        }
        diff.push_str(&format!("\nNew file: {name}\n"));
        for line in content.lines() {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
    }
    if diff.len() > 64_000 {
        bail!("review context exceeds 64000 bytes; split the task");
    }
    let changed_lines = diff
        .lines()
        .filter(|line| {
            (line.starts_with('+') || line.starts_with('-'))
                && !line.starts_with("+++")
                && !line.starts_with("---")
        })
        .count();
    if changed_lines > max_lines {
        bail!("efficiency gate: {changed_lines} changed lines exceeds {max_lines}");
    }
    let candidate = snapshot(directory, &names)?;
    Ok((names, diff, candidate))
}

fn retain_candidate(
    run: &mut Run,
    names: &[String],
    diff: &str,
    candidate: &BTreeMap<String, Fingerprint>,
    workspace: &Path,
    start: &str,
    cancel_file: Option<&Path>,
) -> Result<()> {
    let patch = create_patch(workspace, start, cancel_file)?;
    if patch.is_empty() || patch.len() > 4_000_000 {
        bail!("retained patch must contain the change and be no larger than 4 MB");
    }
    run.stored.write_text("changes.patch", &patch)?;
    let path = run.scratch.join("changes.patch");
    run.state["changed_files"] = json!(names);
    run.state["candidate"] = serde_json::to_value(candidate)?;
    run.state["diff_preview"] = json!(diff);
    run.state["patch_sha256"] = json!(hash_file(&path)?);
    run.state["artifacts"] = json!({
        "patch": path,
        "report": run.scratch.join("run.html"),
    });
    run.persist()
}

fn retain_patch(run: &mut Run, workspace: &Path, cancel_file: Option<&Path>) -> Result<()> {
    let Some(start) = run.state["start"].as_str().map(str::to_owned) else {
        return Ok(());
    };
    if !workspace.is_dir() {
        return Ok(());
    }
    let max_files = run.state["limits"]["max_files"].as_u64().unwrap_or(20) as usize;
    let max_lines = run.state["limits"]["max_lines"].as_u64().unwrap_or(1000) as usize;
    let (names, diff, candidate) = evidence(workspace, &start, max_files, max_lines, cancel_file)?;
    retain_candidate(
        run,
        &names,
        &diff,
        &candidate,
        workspace,
        &start,
        cancel_file,
    )
}

fn create_patch(workspace: &Path, start: &str, cancel_file: Option<&Path>) -> Result<String> {
    let mut patch = git(
        workspace,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            start,
            "--",
        ],
        cancel_file,
    )?;
    if !patch.is_empty() {
        patch.push('\n');
    }
    for name in git(
        workspace,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        cancel_file,
    )?
    .split('\0')
    .filter(|name| !name.is_empty())
    {
        let binary = agent::which("git").ok_or_else(|| anyhow!("install Git"))?;
        let output = agent::execute(
            binary.as_os_str(),
            [
                "diff",
                "--no-index",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-textconv",
                "--",
                "/dev/null",
                name,
            ],
            workspace,
            b"",
            Duration::from_secs(120),
            &BTreeMap::from([("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())]),
            false,
            cancel_file,
        )?;
        if !matches!(output.code, 0 | 1) {
            bail!("Git could not retain an untracked file; inspect the workspace");
        }
        patch.push_str(&output.stdout);
        if !patch.ends_with('\n') {
            patch.push('\n');
        }
        if patch.len() > 4_000_000 {
            bail!("retained patch exceeds 4 MB");
        }
    }
    Ok(patch)
}

fn verification_files(
    directory: &Path,
    checks: &[AcceptanceCheck],
) -> Result<BTreeMap<String, Fingerprint>> {
    let names: Vec<_> = checks
        .iter()
        .flat_map(|check| check.files.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for name in &names {
        let path = directory.join(name);
        if !path.is_file() || path.is_symlink() {
            bail!(
                "acceptance verification file must be an existing regular file without symlinks: {name}"
            );
        }
    }
    snapshot(directory, &names)
}

fn acceptance(
    checks: &[AcceptanceCheck],
    directory: &Path,
    timeout: Duration,
    stored: &StoredRun,
    label: &str,
    cancel_file: Option<&Path>,
) -> Result<Vec<Value>> {
    checks
        .iter()
        .enumerate()
        .map(|(index, check)| {
            let command = agent::split_command(&check.command)?;
            let (status, output) = run_check(&command, directory, timeout, cancel_file)?;
            let name = format!("acceptance-{label}-{}.log", index + 1);
            stored.write_text(&name, &output)?;
            let log = stored.path().join(name);
            let passed = status == i32::from(check.expected_exit)
                && check
                    .expected_output
                    .as_ref()
                    .is_none_or(|expected| expected == &output);
            Ok(json!({
                "criterion": check.criterion, "command": check.command,
                "exit_code": status, "expected_exit": check.expected_exit,
                "output_tail": tail(&output, 1500), "status": if passed { "pass" } else { "fail" },
                "log": log,
            }))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn unchanged(
    workspace: &Path,
    start: &str,
    branch: &str,
    expected_head: &str,
    expected: &BTreeMap<String, Fingerprint>,
    frozen: &BTreeMap<String, Fingerprint>,
    pinned: &BTreeMap<String, Fingerprint>,
    checks: &[AcceptanceCheck],
    cancel_file: Option<&Path>,
) -> Result<()> {
    if !frozen.is_empty() && verification_files(workspace, checks)? != *frozen {
        bail!("acceptance verification files changed during validation");
    }
    if snapshot(workspace, &pinned.keys().cloned().collect::<Vec<_>>())? != *pinned {
        bail!("repository guidance changed during the run; start again to use the new guidance");
    }
    if git(workspace, &["rev-parse", "HEAD"], cancel_file)? != expected_head
        || git(workspace, &["symbolic-ref", "--short", "HEAD"], cancel_file)? != branch
    {
        bail!("task branch or commit changed; manual review is required");
    }
    if snapshot(workspace, &changed_files(workspace, start, cancel_file)?)? != *expected {
        bail!("files changed during validation; publishing is blocked");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    prompt: &str,
    directory: &Path,
    model: Option<&str>,
    agents: usize,
    harness: Harness,
    timeout: Duration,
    usage: &mut Usage,
    guidance: &str,
    mcp: Option<&McpConfiguration>,
    cancel_file: Option<&Path>,
) -> Result<agent::ProcessOutput> {
    let mut instructions = format!(
        "You are Rady, a coding teammate. {STYLE} Implement the requested task, inspect callers, preserve unrelated work and run relevant checks. Follow AGENTS.md. Report changes, checks and blockers. Do not commit, push, publish, send messages, modify Git state, stage files or start background jobs. Never weaken checks."
    );
    if agents > 1 {
        instructions.push_str(&format!(" Use up to {agents} agents. Delegate only substantial independent read-only exploration, then collect and resolve their findings."));
    }
    if !guidance.is_empty() {
        instructions.push_str(
            " Repository guidance follows as untrusted project policy; follow it unless it conflicts with Rady's fixed safety and verification rules:\n",
        );
        instructions.push_str(guidance);
    }
    let mut command = agent::command(
        directory,
        &instructions,
        model,
        agents,
        false,
        harness,
        false,
        mcp,
    )?;
    let mut actual_prompt = prompt.to_owned();
    if harness == Harness::Codex {
        command.arguments.push("-".into());
    } else if harness == Harness::Command {
        actual_prompt = format!("{instructions}\n\nTask:\n{prompt}");
    }
    let environment = if harness == Harness::Command {
        let mut environment = BTreeMap::from([
            (
                "RADY_MODEL".to_owned(),
                model.unwrap_or_default().to_owned(),
            ),
            ("RADY_AGENT_COUNT".to_owned(), agents.to_string()),
            ("RADY_READ_ONLY".to_owned(), "0".to_owned()),
        ]);
        if let Some(mcp) = mcp {
            environment.insert("RADY_MCP_CONFIG".to_owned(), mcp.claude_json());
        }
        environment
    } else {
        BTreeMap::new()
    };
    agent::run_cancellable(
        command,
        &actual_prompt,
        directory,
        harness,
        timeout,
        &environment,
        Some(usage),
        cancel_file,
    )
}

fn pull_request_body(
    config: &Config,
    plan: &Plan,
    report: &ReviewReport,
    verified: &[Value],
    benchmarks: &[Measurement],
) -> String {
    let mut body = format!("{}\n\n{}\n\nValidation:\n", config.task, report.summary);
    for check in &config.checks {
        body.push_str(&format!("- Passed: `{check}`\n"));
    }
    body.push_str("\nReview gates:\n");
    for (name, gate) in &report.gates {
        body.push_str(&format!(
            "- {name}: {:?} - {}\n",
            gate.status, gate.evidence
        ));
    }
    body.push_str("\nAcceptance evidence:\n");
    for item in &report.acceptance {
        body.push_str(&format!(
            "- {}: {:?} - {}\n",
            plan.acceptance[item.criterion], item.status, item.evidence
        ));
    }
    body.push_str("\nFixed acceptance checks:\n");
    for item in verified {
        body.push_str(&format!(
            "- Criterion {}: `{}` - {}\n",
            item["criterion"].as_u64().unwrap_or(0) + 1,
            item["command"].as_str().unwrap_or_default(),
            item["status"].as_str().unwrap_or("unknown")
        ));
    }
    body.push_str(&format!(
        "\nBenchmark: {}\n",
        if benchmarks.is_empty() {
            "Not run; no benchmark supplied".to_owned()
        } else {
            serde_json::to_string(benchmarks).unwrap_or_else(|_| "Unavailable".to_owned())
        }
    ));
    body.push_str("\nLimitations:\n");
    for limitation in report.limitations.iter().chain(&plan.limitations) {
        body.push_str(&format!("- {limitation}\n"));
    }
    body
}

fn report_markdown(state: &Value, include_diff: bool) -> String {
    let mut text = String::new();
    if let Some(id) = state["id"].as_str() {
        text.push_str(&format!(
            "## Run\n\n**{}** · {}\n\n",
            markdown_text(id),
            markdown_text(state["status"].as_str().unwrap_or("unknown"))
        ));
    }
    if cancellation_pending(state) {
        text.push_str(
            "## Cancellation requested\n\nStopping at the current safe boundary; retained evidence remains available.\n\n",
        );
    }
    if let Some(task) = state["task"].as_str() {
        text.push_str(&format!("## The work\n\n> {}\n\n", markdown_text(task)));
    }
    if let Some(acceptance) = state["plan"]["acceptance"].as_array() {
        text.push_str("## Acceptance\n\n");
        for criterion in acceptance.iter().filter_map(Value::as_str) {
            text.push_str(&format!("- {}\n", markdown_text(criterion)));
        }
        text.push('\n');
    }
    if let Some(checks) = state["checks"].as_array() {
        text.push_str("## Checks\n\n");
        for check in checks {
            let (mark, status) = if check["exit_code"] == 0 {
                ("x", "Passed")
            } else {
                (" ", "Failed")
            };
            text.push_str(&format!(
                "- [{mark}] **{status}:** {}\n",
                markdown_text(check["command"].as_str().unwrap_or_default())
            ));
            if include_diff {
                if let Some(log) = check["log"].as_str() {
                    text.push_str(&format!("  - Evidence: {}\n", markdown_text(log)));
                }
            }
        }
        text.push('\n');
    }
    if include_diff {
        if let Some(runs) = state["agent_runs"]
            .as_array()
            .filter(|runs| !runs.is_empty())
        {
            text.push_str("## Agent work\n\n");
            for run in runs {
                text.push_str(&format!(
                    "- **{}:** {}\n",
                    markdown_text(run["outcome"].as_str().unwrap_or("unknown")),
                    markdown_text(run["role"].as_str().unwrap_or("worker"))
                ));
                if let Some(message) = run["message"].as_str() {
                    for line in message.lines() {
                        text.push_str(&format!("  > {}\n", markdown_text(line)));
                    }
                }
                if let Some(log) = run["log"].as_str() {
                    text.push_str(&format!("  - Log: {}\n", markdown_text(log)));
                }
            }
            text.push('\n');
        }
    }
    if include_diff {
        let acceptance = state["acceptance_after"]
            .as_array()
            .or_else(|| state["acceptance_before"].as_array())
            .filter(|acceptance| !acceptance.is_empty());
        if let Some(acceptance) = acceptance {
            text.push_str("## Fixed acceptance evidence\n\n");
            for item in acceptance {
                text.push_str(&format!(
                    "- **{}:** criterion {} · {}\n",
                    markdown_text(item["status"].as_str().unwrap_or("unknown")),
                    item["criterion"].as_u64().unwrap_or(0) + 1,
                    markdown_text(item["command"].as_str().unwrap_or_default())
                ));
                if let Some(log) = item["log"].as_str() {
                    text.push_str(&format!("  - Evidence: {}\n", markdown_text(log)));
                }
            }
            text.push('\n');
        }
    }
    if let Some(review) = state["review"].as_object() {
        text.push_str("## Independent review\n\n");
        text.push_str(&markdown_text(
            review
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ));
        text.push_str("\n\n");
    }
    if let Some(files) = state["changed_files"].as_array() {
        text.push_str("## Changed files\n\n");
        for file in files.iter().filter_map(Value::as_str) {
            text.push_str(&format!("- {}\n", markdown_text(file)));
        }
        text.push('\n');
    }
    if include_diff {
        if let Some(diff) = state["diff_preview"].as_str() {
            text.push_str("## Diff\n\n");
            for line in diff.lines() {
                text.push_str("    ");
                text.push_str(line);
                text.push('\n');
            }
            text.push('\n');
        }
    }
    if let Some(artifacts) = state["artifacts"].as_object() {
        text.push_str("## Artifacts\n\n");
        for (name, path) in artifacts {
            if let Some(path) = path.as_str() {
                text.push_str(&format!(
                    "- **{}:** {}\n",
                    markdown_text(name),
                    markdown_text(path)
                ));
            }
        }
        text.push('\n');
    }
    if let Some(error) = state["error"].as_str() {
        text.push_str("## Needs attention\n\n");
        for line in error.lines() {
            text.push_str(&format!("> {}\n", markdown_text(line)));
        }
        if let Some(publication) = state["publication"].as_object() {
            text.push_str(&format!(
                "\n> Publication status is **{}** for `{}` on branch `{}`. Inspect the remote branch before retrying.\n",
                markdown_text(
                    publication
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                ),
                markdown_text(
                    publication
                        .get("repository")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                ),
                markdown_text(
                    publication
                        .get("branch")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )
            ));
            if let Some(url) = publication.get("url").and_then(Value::as_str) {
                text.push_str(&format!(
                    "\n> Confirmed pull request: `{}`\n",
                    markdown_text(url)
                ));
            }
        }
        if let Some(retention_error) = state["retention_error"].as_str() {
            text.push_str(&format!(
                "\n> The workspace remains available, but a fresh recovery patch could not be retained: {}\n",
                markdown_text(retention_error)
            ));
        }
    }
    text
}

fn markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '('
                | ')'
                | '<'
                | '>'
                | '#'
                | '!'
                | '|'
                | '~'
                | '^'
                | '$'
                | '+'
                | '-'
                | '.'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
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
mod tests {
    use super::*;

    #[test]
    fn repository_remote_parser_covers_supported_and_rejected_forms() {
        for (remote, expected) in [
            ("https://github.com/owner/repo.git", Some("owner/repo")),
            ("git@github.com:owner/repo.git", Some("owner/repo")),
            ("ssh://git@github.com/owner/repo", Some("owner/repo")),
            ("https://example.com/owner/repo", None),
        ] {
            assert_eq!(repository_from_remote(remote).as_deref(), expected);
        }
    }

    #[test]
    fn pinned_base_reference_parser_fails_closed() {
        let reference = "refs/heads/main";
        for (output, expected, accepted) in [
            (
                "0123456789abcdef0123456789abcdef01234567\trefs/heads/main",
                "0123456789abcdef0123456789abcdef01234567",
                true,
            ),
            (
                "0123456789abcdef0123456789abcdef01234567\trefs/heads/main",
                "fedcba9876543210fedcba9876543210fedcba98",
                false,
            ),
            (
                "0123456789abcdef0123456789abcdef01234567\trefs/heads/other",
                "0123456789abcdef0123456789abcdef01234567",
                false,
            ),
            (
                "0123456789abcdef0123456789abcdef01234567\trefs/heads/main\nextra",
                "0123456789abcdef0123456789abcdef01234567",
                false,
            ),
            (
                "short\trefs/heads/main",
                "0123456789abcdef0123456789abcdef01234567",
                false,
            ),
        ] {
            assert_eq!(
                parse_remote_ref(output, reference).is_ok_and(|object_id| object_id == expected),
                accepted
            );
        }
    }

    #[test]
    fn network_auth_is_ephemeral_and_limited_to_github_https() -> Result<()> {
        let environment = git_network_environment("https://github.com/owner/repo.git", "token")?;
        assert_eq!(environment["GIT_CONFIG_COUNT"], "1");
        assert_eq!(
            environment["GIT_CONFIG_KEY_0"],
            "http.https://github.com/.extraheader"
        );
        assert_eq!(
            environment["GIT_CONFIG_VALUE_0"],
            "AUTHORIZATION: Basic eC1hY2Nlc3MtdG9rZW46dG9rZW4="
        );
        assert!(environment.values().all(|value| !value.contains("token")));
        for (remote, token) in [
            ("git@github.com:owner/repo.git", "token"),
            ("https://example.com/owner/repo.git", "token"),
            ("https://github.com/owner/repo.git", ""),
        ] {
            assert!(git_network_environment(remote, token).is_err());
        }
        assert!(
            git_network_environment(
                "https://github.com/owner/repo.git",
                &"x".repeat(MAX_PUSH_TOKEN_BYTES + 1)
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn command_parser_and_tail_cover_quotes_unicode_and_limits() -> Result<()> {
        let commands = parse_commands(&["cargo test -- 'two words'".to_owned()])?;
        assert_eq!(commands[0], ["cargo", "test", "--", "two words"]);
        assert_eq!(tail("hello", 3), "llo");
        assert_eq!(tail("🦔rust", 4), "rust");
        let state = json!({
            "task": "Fix [unsafe](javascript:alert(1)) <script> $x$ ^power^",
            "error": "Stopped *safely*", "changed_files": ["src/lib.rs"],
            "publication": {"status": "branch pushed", "repository": "owner/repo", "branch": "rady/test"},
            "retention_error": "patch unavailable",
            "diff_preview": "+safe", "checks": [{"command": "check", "exit_code": 0,
                "log": "/tmp/check.log"}]
        });
        let report = report_markdown(&state, true);
        assert!(report.contains(r"\[unsafe\]"));
        assert!(report.contains(r"\<script\>"));
        assert!(report.contains(r"\$x\$"));
        assert!(report.contains(r"\^power\^"));
        assert!(report.contains(r"\*safely\*"));
        assert!(report.contains("Publication status"));
        assert!(report.contains("recovery patch could not be retained"));
        assert!(report.contains("## Diff"));
        assert!(report.contains("Evidence:"));
        assert!(!report_markdown(&state, false).contains("## Diff"));
        Ok(())
    }

    #[test]
    fn retained_state_and_budget_validation_are_table_driven() {
        let id = "run_0123456789abcdef0123456789abcdef";
        for (state, accepted) in [
            (json!({"schema": 1, "id": id, "status": "ready"}), true),
            (json!({"schema": 2, "id": id, "status": "ready"}), false),
            (
                json!({"schema": 1, "id": "run_wrong", "status": "ready"}),
                false,
            ),
            (json!({"schema": 1, "id": id, "status": "unknown"}), false),
        ] {
            assert_eq!(validate_run_state(id, &state).is_ok(), accepted);
        }
        for (maximum, usage, remaining) in [
            (100, json!({"complete": true, "total_tokens": 40}), Some(60)),
            (100, json!({"complete": true, "total_tokens": 100}), None),
            (100, json!({"complete": false, "total_tokens": 40}), None),
            (100, json!({"complete": true}), None),
        ] {
            assert_eq!(remaining_budget(maximum, &usage).ok(), remaining);
        }
    }

    #[test]
    fn cancellation_projection_is_immediate_without_rewriting_status() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let store = RunStore::at(temporary.path())?;
        for (status, requested, pending) in [
            ("active", false, false),
            ("active", true, true),
            ("cancelled", true, false),
        ] {
            let run = store.create()?;
            if requested {
                run.cancel()?;
            }
            let observed = observed_run_state(&run, json!({"status": status}))?;
            assert_eq!(observed["status"], status);
            assert_eq!(observed["cancel_requested"], requested);
            assert_eq!(cancellation_pending(&observed), pending);
        }
        Ok(())
    }

    #[test]
    fn credential_scan_is_bounded_and_never_follows_symlinks() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let credential = Regex::new(CREDENTIAL_PATTERN)?;
        for (name, content, rejected) in [
            ("safe.txt", "ordinary text", false),
            ("secret.txt", "-----BEGIN PRIVATE KEY-----", true),
            (
                "token.txt",
                "github_pat_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                true,
            ),
            ("temporary-key.txt", "ASIAAAAAAAAAAAAAAAAA", true),
            (
                "gemini-legacy.txt",
                "AIzaAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                true,
            ),
            ("gemini-current.txt", "AQ.AAAAAAAAAAAAAAAAAAAA", true),
            ("cerebras.txt", "csk-AAAAAAAAAAAAAAAAAAAA", true),
            ("xai.txt", "xai-AAAAAAAAAAAAAAAAAAAA", true),
        ] {
            let path = temporary.path().join(name);
            fs::write(&path, content)?;
            assert_eq!(
                scan_credentials(temporary.path(), Path::new(name), name, &credential).is_err(),
                rejected
            );
        }

        let external = tempfile::tempdir()?;
        let outside = external.path().join("outside.txt");
        fs::write(&outside, "-----BEGIN PRIVATE KEY-----")?;
        let linked = temporary.path().join("linked.txt");
        std::os::unix::fs::symlink(&outside, &linked)?;
        assert!(
            scan_credentials(
                temporary.path(),
                Path::new("linked.txt"),
                "linked.txt",
                &credential
            )
            .is_err()
        );

        let linked_directory = temporary.path().join("linked-directory");
        std::os::unix::fs::symlink(external.path(), &linked_directory)?;
        assert!(
            scan_credentials(
                temporary.path(),
                Path::new("linked-directory/outside.txt"),
                "linked-directory/outside.txt",
                &credential
            )
            .is_err()
        );

        let large = temporary.path().join("large.txt");
        fs::write(&large, vec![b'x'; MAX_CREDENTIAL_SCAN_BYTES + 1])?;
        assert!(
            scan_credentials(
                temporary.path(),
                Path::new("large.txt"),
                "large.txt",
                &credential
            )
            .is_err()
        );

        let socket = temporary.path().join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket)?;
        assert!(
            scan_credentials(temporary.path(), Path::new("socket"), "socket", &credential).is_err()
        );
        Ok(())
    }

    #[test]
    fn snapshot_uses_safe_workspace_descriptors() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        fs::write(temporary.path().join("regular.txt"), "safe")?;
        std::os::unix::fs::symlink("regular.txt", temporary.path().join("linked.txt"))?;
        let external = tempfile::tempdir()?;
        fs::write(external.path().join("outside.txt"), "outside")?;
        std::os::unix::fs::symlink(external.path(), temporary.path().join("linked-directory"))?;
        let socket = temporary.path().join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket)?;

        for (name, accepted) in [
            ("regular.txt", true),
            ("linked.txt", true),
            ("linked-directory/outside.txt", false),
            ("socket", false),
        ] {
            let result = snapshot(temporary.path(), &[name.to_owned()]);
            assert_eq!(result.is_ok(), accepted, "{name}");
        }
        let linked = snapshot(temporary.path(), &["linked.txt".to_owned()])?;
        assert!(matches!(
            linked.get("linked.txt"),
            Some(Fingerprint::Symlink { .. })
        ));
        Ok(())
    }
}
