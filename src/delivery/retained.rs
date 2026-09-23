use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};
use serde_json::json;

use crate::Result;
use crate::agent::{self, Usage};
use crate::quality::{self, Plan};
use crate::runs::{Run as StoredRun, RunStore};
use crate::ui::{
    OutputMode, ReportState, Theme, Ui, json_success_document, print_markdown, write_report,
};
use serde_json::Value;

use super::Config;
use super::evidence::{Fingerprint, changed_files, evidence, hash_file, snapshot};
use super::git::git;

pub(super) fn retain_candidate(
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

pub(super) fn retain_patch(
    run: &mut Run,
    workspace: &Path,
    cancel_file: Option<&Path>,
) -> Result<()> {
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
            std::time::Duration::from_secs(120),
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
pub(super) struct Run {
    pub(super) scratch: PathBuf,
    pub(super) stored: StoredRun,
    pub(super) state: Value,
    pub(super) usage: Usage,
    pub(super) ui: Ui,
    pub(super) theme: Theme,
    pub(super) output: OutputMode,
}

impl Run {
    pub(super) fn persist(&self) -> Result<()> {
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

    pub(super) fn ensure_active(&self) -> Result<()> {
        if self.stored.is_cancelled()? {
            bail!("run cancelled");
        }
        Ok(())
    }

    pub(super) fn stage(&mut self, name: &str) -> Result<()> {
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

    pub(super) fn finish(&self) -> Result<()> {
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
    super::validate_config(&config)?;
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
    super::deliver(config)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_escapes_untrusted_content_and_hides_diff_when_requested() {
        let state = json!({
            "task": "Fix [unsafe](javascript:alert(1)) <script> $x$ ^power^",
            "error": "Stopped *safely*", "changed_files": ["src/lib.rs"],
            "publication": {"status": "branch pushed", "repository": "owner/repo", "branch": "rady/test"},
            "retention_error": "patch unavailable", "diff_preview": "+safe",
            "checks": [{"command": "check", "exit_code": 0, "log": "/tmp/check.log"}]
        });
        let report = report_markdown(&state, true);
        for escaped in [
            r"\[unsafe\]",
            r"\<script\>",
            r"\$x\$",
            r"\^power\^",
            r"\*safely\*",
        ] {
            assert!(report.contains(escaped));
        }
        assert!(report.contains("Publication status"));
        assert!(report.contains("recovery patch could not be retained"));
        assert!(report.contains("## Diff"));
        assert!(!report_markdown(&state, false).contains("## Diff"));
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
}
