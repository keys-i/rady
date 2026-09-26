use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

use super::quality::AcceptanceCheck;
use crate::Result;
use crate::agent;
use crate::runs::Run as StoredRun;

use super::{Fingerprint, changed_files, git, snapshot, tail};

pub(super) fn run_check(
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

pub(super) fn verification_files(
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

pub(super) fn acceptance(
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
pub(super) fn unchanged(
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
