use std::collections::{BTreeMap, BTreeSet};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::agent::Harness;
use crate::github::GitHub;
use crate::model::{INSTRUCTIONS, ModelReview, Risk, model_review};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewOutcome {
    pub approved: bool,
    pub enable_auto_merge: bool,
}

pub fn resolve(github: &GitHub, number: u64) -> Result<(Value, bool)> {
    let pull = github.api(&format!("pulls/{number}"), None, "GET")?;
    if pull["state"] != "open" || pull["draft"].as_bool() != Some(false) {
        bail!("only open, ready-for-review PRs are supported");
    }
    for path in [["head", "sha"], ["base", "sha"]] {
        let sha = pull[path[0]][path[1]]
            .as_str()
            .ok_or_else(|| anyhow!("invalid PR commit"))?;
        if !is_sha(sha) {
            bail!("invalid PR commit");
        }
    }
    let dependency = pull["user"]["login"] == "dependabot[bot]";
    if dependency && pull["head"]["repo"]["full_name"] != github.repo() {
        bail!("Dependabot updates must originate in the target repository");
    }
    Ok((pull, dependency))
}

pub fn checks(github: &GitHub, head: &str) -> Result<Vec<Value>> {
    let runs = github.pages(
        &format!("commits/{head}/check-runs?filter=latest"),
        Some("check_runs"),
    )?;
    let statuses = github.pages(&format!("commits/{head}/statuses"), None)?;
    let mut latest = BTreeMap::<(String, String), Value>::new();
    for run in runs {
        let name = text(&run, &["name"])?;
        let app_id = run["app"]["id"].to_string();
        let key = (name.to_owned(), app_id);
        let id = integer(&run, &["id"])?;
        let replace = latest
            .get(&key)
            .and_then(|value| value["id"].as_i64())
            .is_none_or(|current| id > current);
        if replace {
            let state = if run["status"] == "completed" {
                run["conclusion"].clone()
            } else {
                run["status"].clone()
            };
            latest.insert(
                key,
                json!({
                    "id": id,
                    "name": name,
                    "app_id": run["app"]["id"],
                    "state": state,
                    "url": run["html_url"].as_str().unwrap_or_default(),
                    "detail": run["output"]["summary"].as_str().unwrap_or_default().chars().take(3000).collect::<String>()
                }),
            );
        }
    }
    for status in statuses {
        let name = text(&status, &["context"])?;
        let key = (name.to_owned(), "status".to_owned());
        latest.entry(key).or_insert_with(|| {
            json!({
                "id": status["id"],
                "name": name,
                "app_id": Value::Null,
                "state": status["state"],
                "url": status["target_url"].as_str().unwrap_or_default(),
                "detail": status["description"].as_str().unwrap_or_default().chars().take(3000).collect::<String>()
            })
        });
    }
    let mut rows: Vec<_> = latest.into_values().collect();
    rows.sort_by(|left, right| {
        (left["name"].as_str(), left["id"].as_i64())
            .cmp(&(right["name"].as_str(), right["id"].as_i64()))
    });
    Ok(rows)
}

pub fn ci_blockers(
    rows: &[Value],
    required: &[String],
    protection: Option<&Value>,
) -> Result<Vec<String>> {
    let status = protection
        .and_then(|value| value.get("required_status_checks"))
        .unwrap_or(&Value::Null);
    let bindings: BTreeMap<String, i64> = status["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some((
                item["context"].as_str()?.to_owned(),
                item["app_id"].as_i64().unwrap_or(-1),
            ))
        })
        .collect();
    let contexts: BTreeSet<String> = status["contexts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect();
    let names: BTreeSet<_> = required
        .iter()
        .chain(contexts.iter())
        .chain(bindings.keys())
        .cloned()
        .collect();
    let mut blockers = Vec::new();
    for name in names {
        let matching: Vec<_> = rows
            .iter()
            .filter(|row| {
                row["name"] == name
                    && bindings.get(&name).is_none_or(|app_id| {
                        *app_id == -1 || row["app_id"].as_i64() == Some(*app_id)
                    })
            })
            .collect();
        if matching.is_empty() {
            blockers.push(format!("`{name}` hasn't reported for this commit yet"));
        } else if matching.iter().any(|row| row["state"] != "success") {
            let states = matching
                .iter()
                .map(|row| row["state"].as_str().unwrap_or("unknown"))
                .collect::<Vec<_>>()
                .join(", ");
            if matching.iter().all(|row| {
                matches!(
                    row["state"].as_str(),
                    Some("queued" | "in_progress" | "pending" | "waiting" | "requested")
                )
            }) {
                blockers.push(format!("`{name}` is still running ({states})"));
            } else {
                blockers.push(format!("`{name}` needs attention ({states})"));
            }
        }
    }
    Ok(blockers)
}

fn auto_merge_ready(protection: Option<&Value>) -> bool {
    let Some(protection) = protection else {
        return false;
    };
    let status = &protection["required_status_checks"];
    protection["enforce_admins"]["enabled"].as_bool() == Some(true)
        && status["strict"].as_bool() == Some(true)
        && (status["contexts"]
            .as_array()
            .is_some_and(|contexts| !contexts.is_empty())
            || status["checks"]
                .as_array()
                .is_some_and(|checks| !checks.is_empty()))
}

pub fn files_context(github: &GitHub, number: u64, count: usize) -> Result<(Vec<Value>, bool)> {
    let files = github.pages(&format!("pulls/{number}/files"), None)?;
    let mut evidence = Vec::with_capacity(files.len());
    let mut complete = files.len() == count;
    let mut remaining = 160_000_usize;
    for file in files {
        let patch = file["patch"].as_str().unwrap_or_default();
        let covered = patch_evidence_complete(&file, remaining);
        complete &= covered;
        let clipped = if patch.len() <= remaining {
            patch
        } else {
            patch.get(..remaining).unwrap_or_default()
        };
        let mut item = json!({
            "filename": file["filename"], "status": file["status"],
            "additions": file["additions"], "deletions": file["deletions"],
            "changes": file["changes"], "patch": clipped, "complete": covered
        });
        if !file["previous_filename"].is_null() {
            item["previous_filename"] = file["previous_filename"].clone();
        }
        evidence.push(item);
        remaining = remaining.saturating_sub(patch.len());
    }
    Ok((evidence, complete))
}

fn patch_evidence_complete(file: &Value, remaining: usize) -> bool {
    let Some(patch) = file["patch"].as_str().filter(|patch| !patch.is_empty()) else {
        return false;
    };
    if patch.len() > remaining {
        return false;
    }
    let (Some(additions), Some(deletions)) = (
        file["additions"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok()),
        file["deletions"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok()),
    ) else {
        return false;
    };
    let mut additions_seen = 0;
    let mut deletions_seen = 0;
    let mut in_hunk = false;
    let mut saw_hunk = false;
    for line in patch.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            saw_hunk = true;
        } else if in_hunk && line.starts_with('+') {
            additions_seen += 1;
        } else if in_hunk && line.starts_with('-') {
            deletions_seen += 1;
        }
    }
    saw_hunk && additions_seen == additions && deletions_seen == deletions
}

pub fn compatibility(raw: &str) -> Option<f64> {
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
}

pub(crate) fn allowed_dependency_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return false;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    let name = parts.last().copied().unwrap_or_default();
    if matches!(parts.as_slice(), [".github", "workflows", workflow] if workflow.ends_with(".yml") || workflow.ends_with(".yaml"))
    {
        return true;
    }
    matches!(
        name,
        "Cargo.toml"
            | "Cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "npm-shrinkwrap.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "bun.lock"
            | "bun.lockb"
            | "Pipfile"
            | "Pipfile.lock"
            | "pyproject.toml"
            | "poetry.lock"
            | "uv.lock"
            | "pdm.lock"
            | "go.mod"
            | "go.sum"
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "settings.gradle"
            | "settings.gradle.kts"
            | "gradle-wrapper.properties"
            | "Gemfile"
            | "Gemfile.lock"
            | "composer.json"
            | "composer.lock"
            | "Package.swift"
            | "Package.resolved"
            | "mix.exs"
            | "mix.lock"
            | "pubspec.yaml"
            | "pubspec.lock"
            | "renv.lock"
            | "DESCRIPTION"
            | "packages.config"
            | "packages.lock.json"
            | "Directory.Packages.props"
            | "libs.versions.toml"
            | "Dockerfile"
    ) || name.starts_with("requirements") && name.ends_with(".txt")
        || name.starts_with("constraints") && name.ends_with(".txt")
        || name.ends_with(".csproj")
        || name.ends_with(".fsproj")
        || name.ends_with(".vbproj")
}

fn dependency_file_blockers(files: &[Value]) -> Vec<String> {
    if files.is_empty() {
        return vec!["Dependabot changed no inspectable files; review it manually".to_owned()];
    }
    let unexpected = files.iter().any(|file| {
        !file["filename"]
            .as_str()
            .is_some_and(allowed_dependency_path)
            || file["previous_filename"]
                .as_str()
                .is_some_and(|path| !allowed_dependency_path(path))
    });
    unexpected
        .then(|| {
            "Dependabot changed files outside approved dependency files; review it manually"
                .to_owned()
        })
        .into_iter()
        .collect()
}

pub fn decision(
    review: &ModelReview,
    context: &Value,
    required: &[String],
    protection: Option<&Value>,
) -> Result<(&'static str, Vec<String>)> {
    let mut blockers = review.blockers.clone();
    blockers.extend(ci_blockers(
        context["checks"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
        required,
        protection,
    )?);
    if context["complete_diff"].as_bool() != Some(true) {
        blockers.push("Some changes are binary, too large or missing from the supplied diff; review those manually".to_owned());
    }
    if context["dependency"].as_bool() == Some(true) {
        blockers.extend(dependency_file_blockers(
            context["files"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        ));
        match context["score"].as_f64() {
            None => blockers.push("Compatibility is unavailable; it needs manual review".to_owned()),
            Some(score) if score < 80.0 => blockers.push(format!("Compatibility is {score}%, below 80%; investigate checks and upstream release notes")),
            Some(score) if score < 95.0 => blockers.push(format!("Compatibility is {score}%; automatic approval requires at least 95%")),
            Some(_) => {}
        }
        if !matches!(
            context["update_type"].as_str(),
            Some("version-update:semver-patch" | "version-update:semver-minor")
        ) {
            blockers.push(
                "This is not a verified patch or minor update; review it manually".to_owned(),
            );
        }
        if context["maintainer_changes"] != "false" {
            blockers.push(
                "Maintainer changes have not been ruled out; check package ownership".to_owned(),
            );
        }
    }
    blockers.sort();
    blockers.dedup();
    let event = if blockers.is_empty() && review.risk == Risk::Low {
        "APPROVE"
    } else {
        "COMMENT"
    };
    Ok((event, blockers))
}

fn ci_wait_only(blockers: &[String]) -> bool {
    !blockers.is_empty()
        && blockers.iter().all(|blocker| {
            blocker.starts_with('`')
                && (blocker.contains(" hasn't reported for this commit yet")
                    || blocker.contains(" is still running ("))
        })
}

fn repair_handoff(
    lines: &mut Vec<String>,
    review: &ModelReview,
    context: &Value,
    blockers: &[String],
) {
    if ci_wait_only(blockers) {
        return;
    }

    let number = context["number"]
        .as_u64()
        .map_or_else(|| "unknown".to_owned(), |value| format!("#{value}"));
    let url = context["url"]
        .as_str()
        .filter(|value| value.starts_with("https://"));
    let source = url.map_or(number.clone(), |value| format!("{number} {value}"));
    let head = context["head"].as_str().unwrap_or("unknown");
    let mut unique = BTreeSet::new();
    unique.extend(
        blockers
            .iter()
            .filter(|blocker| !blocker.trim().is_empty())
            .cloned(),
    );

    lines.extend([
        "Resolve the blockers below. If a change is required, create a maintainer replacement PR; do not push to the Dependabot branch."
            .to_owned(),
        String::new(),
        "Repair brief".to_owned(),
        format!("- Source: {}", markdown_text(&source)),
        format!("- Reviewed head: {}", markdown_text(head)),
        format!("- Review risk: {:?}", review.risk),
    ]);
    if let Some(score) = context["score"].as_f64() {
        lines.push(format!("- Compatibility: {score}%"));
    }
    if let Some(update_type) = context["update_type"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("- Update type: {}", markdown_text(update_type)));
    }
    if let Some(maintainer_changes) = context["maintainer_changes"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!(
            "- Maintainer changes: {}",
            markdown_text(maintainer_changes)
        ));
    }
    lines.extend([
        "- Allowed files: approved manifests, lockfiles, Actions workflows and Dockerfiles only"
            .to_owned(),
        "- Required outcome: every configured and protected check passes on this commit".to_owned(),
        "- Blockers:".to_owned(),
    ]);
    lines.extend(unique.into_iter().take(12).map(|blocker| {
        format!(
            "  - {}",
            markdown_text(&blocker.chars().take(500).collect::<String>())
        )
    }));
}

fn safe_https_link(label: &str, url: &str) -> String {
    let label = markdown_text(label);
    if url.starts_with("https://")
        && !url.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || character == '<'
                || character == '>'
        })
    {
        format!("[{label}](<{url}>)")
    } else {
        label
    }
}

fn ci_snapshot(checks: &[Value]) -> String {
    let successful = checks
        .iter()
        .filter(|check| check["state"].as_str() == Some("success"))
        .count();
    format!("{successful}/{} checks successful", checks.len())
}

fn next_action(event: &str, blockers: &[String]) -> &'static str {
    if event == "APPROVE" {
        "Review the Files changed tab, then use GitHub's Merge control when it is enabled"
    } else if ci_wait_only(blockers) {
        "Wait for the named checks to finish on the reviewed head, then rerun Rady"
    } else {
        "Resolve the blockers below, then rerun Rady on the new head"
    }
}

pub fn render(
    review: &ModelReview,
    context: &Value,
    event: &str,
    blockers: &[String],
    marker: &str,
) -> String {
    let name = if context["dependency"].as_bool() == Some(true) {
        "Dependasolver"
    } else {
        "Rady"
    };
    let ready = event == "APPROVE";
    let head = context["head"].as_str().unwrap_or_default();
    let checks = context["checks"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let files = context["files"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut lines = vec![
        marker.to_owned(),
        format!(
            "### {name} — {}",
            if ready { "Ready to merge" } else { "Hold" }
        ),
        String::new(),
        format!(
            "Reviewed head: `{}` · CI snapshot: {}",
            head.chars().take(12).collect::<String>(),
            ci_snapshot(checks)
        ),
        String::new(),
    ];
    if ready {
        lines.push("The supplied diff and required check evidence are approved. GitHub remains the source of truth for mergeability.".to_owned());
    } else {
        lines.push("This review is not approving the PR yet.".to_owned());
    }
    lines.extend([String::new(), "What changed".to_owned()]);
    if files.is_empty() {
        lines.push("- No changed-file evidence was supplied".to_owned());
    } else {
        lines.extend(files.iter().take(12).map(|file| {
            format!(
                "- `{}` · +{} −{}",
                markdown_text(file["filename"].as_str().unwrap_or("unknown")),
                file["additions"].as_u64().unwrap_or_default(),
                file["deletions"].as_u64().unwrap_or_default(),
            )
        }));
        if files.len() > 12 {
            lines.push(format!("- …and {} more files", files.len() - 12));
        }
    }
    lines.extend([String::new(), "Checks".to_owned()]);
    if checks.is_empty() {
        lines.push("- No GitHub check evidence was supplied".to_owned());
    } else {
        lines.extend(checks.iter().take(20).map(|item| {
            format!(
                "- {} — {}",
                safe_https_link(
                    item["name"].as_str().unwrap_or("unknown"),
                    item["url"].as_str().unwrap_or_default()
                ),
                markdown_text(item["state"].as_str().unwrap_or("unknown"))
            )
        }));
        if checks.len() > 20 {
            lines.push(format!("- …and {} more checks", checks.len() - 20));
        }
    }
    lines.extend([
        String::new(),
        "Next action".to_owned(),
        next_action(event, blockers).to_owned(),
        String::new(),
        markdown_text(&review.summary),
    ]);
    if !review.observations.is_empty() {
        lines.extend([String::new(), "Notes".to_owned()]);
    }
    lines.extend(
        review
            .observations
            .iter()
            .map(|item| format!("- {}", markdown_text(item))),
    );
    if context["dependency"].as_bool() == Some(true) {
        lines.extend([
            String::new(),
            format!(
                "Compatibility - {}",
                context["score"]
                    .as_f64()
                    .map_or_else(|| "unavailable".to_owned(), |score| format!("{score}%"))
            ),
        ]);
    }
    if !blockers.is_empty() {
        lines.extend([String::new(), "Before merge".to_owned()]);
        lines.extend(
            blockers
                .iter()
                .map(|item| format!("- {}", markdown_text(item))),
        );
    }
    if event != "APPROVE" && context["dependency"].as_bool() == Some(true) {
        repair_handoff(&mut lines, review, context, blockers);
    }
    if !review.minor.is_empty() {
        lines.extend([String::new(), "Small things".to_owned()]);
        lines.extend(
            review
                .minor
                .iter()
                .map(|item| format!("- {}", markdown_text(item))),
        );
    }
    lines.extend([
        String::new(),
        format!("Review - {:?} risk", review.risk),
        String::new(),
        "Reviewed the supplied diff and GitHub check results; this reviewer ran no tests."
            .to_owned(),
    ]);
    let body = lines[1..]
        .join("\n")
        .replace('@', "@\u{200b}")
        .replace("<!--", "&lt;!--");
    format!("{marker}\n{body}")
}

fn markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            escaped.push(' ');
            continue;
        }
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '!' | '|' | '~' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[allow(clippy::too_many_arguments)]
pub fn review_pr(
    github: &GitHub,
    number: u64,
    required: &[String],
    model: Option<&str>,
    bot_slug: &str,
    harness: Harness,
    score: &str,
    update_type: &str,
    maintainer_changes: &str,
    expected_head: &str,
    wait: Duration,
) -> Result<ReviewOutcome> {
    let slug = Regex::new(r"^[a-z0-9-]+$")?;
    if !slug.is_match(bot_slug)
        || required
            .iter()
            .any(|name| name.trim().is_empty() || name.starts_with("Rady dependasolve"))
    {
        bail!("selected App slug and valid external required checks are required");
    }
    let (pull, dependency) = resolve(github, number)?;
    let head = text(&pull, &["head", "sha"])?;
    if !expected_head.is_empty() && head != expected_head {
        bail!("the PR changed since this workflow started; rerun on the new commit");
    }
    let changed_files = pull["changed_files"].as_u64().unwrap_or_default() as usize;
    let (files, complete) = files_context(github, number, changed_files)?;
    let base_ref = text(&pull, &["base", "ref"])?;
    let endpoint = format!("branches/{}/protection", percent_encode(base_ref));
    let mut protection = github.api_optional(&endpoint, None, "GET")?;
    let mut rows = checks(github, head)?;
    let deadline = Instant::now() + wait;
    let names: BTreeSet<_> = required.iter().map(String::as_str).collect();
    while Instant::now() < deadline
        && (names
            .iter()
            .any(|name| !rows.iter().any(|row| row["name"] == **name))
            || rows.iter().any(|row| {
                names.contains(row["name"].as_str().unwrap_or_default())
                    && matches!(
                        row["state"].as_str(),
                        Some("queued" | "in_progress" | "pending" | "waiting" | "requested")
                    )
            }))
    {
        thread::sleep(
            Duration::from_secs(10).min(deadline.saturating_duration_since(Instant::now())),
        );
        rows = checks(github, head)?;
    }
    let context = json!({
        "number": number, "url": pull["html_url"],
        "head": head, "base": pull["base"]["sha"],
        "title": pull["title"].as_str().unwrap_or_default().chars().take(2000).collect::<String>(),
        "description": pull["body"].as_str().unwrap_or_default().chars().take(12000).collect::<String>(),
        "dependency": dependency, "files": files, "complete_diff": complete,
        "checks": rows, "score": compatibility(score), "update_type": update_type,
        "maintainer_changes": maintainer_changes,
    });
    let fingerprint = Sha256::digest(serde_json::to_vec(&json!([
        context,
        required,
        protection,
        harness.as_str(),
        model,
        INSTRUCTIONS
    ]))?);
    let marker = format!("<!-- rady-review-{} -->", hex(&fingerprint[..12]));
    let reviews = github.pages(&format!("pulls/{number}/reviews"), None)?;
    let login = format!("{bot_slug}[bot]");
    let own: Vec<_> = reviews
        .iter()
        .filter(|item| item["user"]["login"] == login)
        .collect();
    if let Some(previous) = own.iter().find(|item| {
        item["body"]
            .as_str()
            .is_some_and(|body| body.contains(&marker))
            && item["commit_id"] == context["head"]
            && item["state"] != "DISMISSED"
    }) {
        println!("This commit and CI state already have a review");
        let approved = previous["state"] == "APPROVED";
        return Ok(ReviewOutcome {
            approved,
            enable_auto_merge: dependency && approved && auto_merge_ready(protection.as_ref()),
        });
    }
    for previous in own.into_iter().filter(|item| item["state"] == "APPROVED") {
        github.api(
            &format!("pulls/{number}/reviews/{}/dismissals", previous["id"]),
            Some(&json!({"message": "Rechecking the current diff and CI results"})),
            "PUT",
        )?;
    }
    let result = model_review(&context, model, harness)?;
    let (current, _) = resolve(github, number)?;
    if current["head"]["sha"] != context["head"] || current["base"]["sha"] != context["base"] {
        bail!("the PR changed during review; rerun on the new commit");
    }
    if json!(checks(github, head)?) != context["checks"] {
        bail!("CI changed during review; rerun to assess the latest results");
    }
    protection = github.api_optional(&endpoint, None, "GET")?;
    let (event, blockers) = decision(&result, &context, required, protection.as_ref())?;
    let body = render(&result, &context, event, &blockers, &marker);
    github.api(
        &format!("pulls/{number}/reviews"),
        Some(&json!({"commit_id": head, "event": event, "body": body})),
        "POST",
    )?;
    println!(
        "Published {} for {}",
        event.to_ascii_lowercase(),
        head.chars().take(12).collect::<String>()
    );
    let approved = event == "APPROVE";
    Ok(ReviewOutcome {
        approved,
        enable_auto_merge: approved && dependency && auto_merge_ready(protection.as_ref()),
    })
}

fn text<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str> {
    path.iter()
        .try_fold(value, |current, key| {
            current.get(*key).ok_or_else(|| anyhow!("missing {key}"))
        })?
        .as_str()
        .ok_or_else(|| anyhow!("expected text at {}", path.join(".")))
}

fn integer(value: &Value, path: &[&str]) -> Result<i64> {
    path.iter()
        .try_fold(value, |current, key| {
            current.get(*key).ok_or_else(|| anyhow!("missing {key}"))
        })?
        .as_i64()
        .ok_or_else(|| anyhow!("expected integer at {}", path.join(".")))
}

fn is_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
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
    fn table_driven_compatibility_and_sha_validation() {
        for (value, expected) in [
            ("95", Some(95.0)),
            ("100.1", None),
            ("bad", None),
            ("-1", None),
        ] {
            assert_eq!(compatibility(value), expected);
        }
        for (value, valid) in [
            ("a".repeat(40), true),
            ("A".repeat(40), false),
            ("a".repeat(39), false),
        ] {
            assert_eq!(is_sha(&value), valid);
        }
    }

    #[test]
    fn ci_evidence_is_independent_from_auto_merge_protection() -> Result<()> {
        let success = |name: &str| json!({"name": name, "state": "success", "app_id": null});
        let protected = json!({
            "enforce_admins": {"enabled": true},
            "required_status_checks": {"strict": true, "contexts": ["protected"], "checks": []}
        });
        let unprotected = json!({
            "enforce_admins": {"enabled": false},
            "required_status_checks": {"strict": false, "contexts": [], "checks": []}
        });
        for (protection, required, rows, blockers, auto_merge) in [
            (None, vec!["test"], vec![success("test")], 0, false),
            (
                Some(unprotected),
                vec!["test"],
                vec![success("test")],
                0,
                false,
            ),
            (
                Some(protected),
                vec!["test"],
                vec![success("test"), success("protected")],
                0,
                true,
            ),
            (
                Some(json!({"required_status_checks": {"contexts": ["protected"]}})),
                vec!["test"],
                vec![success("test")],
                1,
                false,
            ),
        ] {
            assert_eq!(
                ci_blockers(
                    &rows,
                    &required.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    protection.as_ref()
                )?
                .len(),
                blockers
            );
            assert_eq!(auto_merge_ready(protection.as_ref()), auto_merge);
        }
        Ok(())
    }

    #[test]
    fn patch_evidence_fails_closed_when_the_diff_is_absent_or_incomplete() {
        for (file, remaining, complete) in [
            (json!({}), 100, false),
            (json!({"patch": null}), 100, false),
            (json!({"patch": 4}), 100, false),
            (
                json!({"patch": "", "additions": 0, "deletions": 0}),
                100,
                false,
            ),
            (json!({"patch": "@@ -1 +1 @@\n unchanged"}), 100, false),
            (
                json!({"patch": "Binary files differ", "additions": 0, "deletions": 0}),
                100,
                false,
            ),
            (
                json!({"patch": "@@ -1 +1 @@\n-old\n+new", "additions": 1, "deletions": 1}),
                100,
                true,
            ),
            (
                json!({"patch": "@@ -1 +1 @@\n-old\n+new", "additions": 1, "deletions": 1}),
                3,
                false,
            ),
        ] {
            assert_eq!(
                patch_evidence_complete(&file, remaining),
                complete,
                "{file}"
            );
        }
    }

    #[test]
    fn rendering_neutralises_mentions_and_hidden_markers() {
        let review = ModelReview {
            summary: "Reviewed. @person <!-- marker [click](https://example.test)\n# fake"
                .to_owned(),
            risk: Risk::Low,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let body = render(
            &review,
            &json!({"dependency": false, "head": "a".repeat(40), "checks": []}),
            "COMMENT",
            &[],
            "<!-- safe -->",
        );
        assert!(body.contains("@\u{200b}person"));
        assert!(!body["<!-- safe -->".len()..].contains("<!--"));
        assert!(body.contains(r"\[click\]"));
        assert!(!body.contains("\n# fake"));
    }

    #[test]
    fn render_leads_with_a_bounded_evidence_snapshot_and_next_action() {
        let review = ModelReview {
            summary: "Evidence summary".to_owned(),
            risk: Risk::Low,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let context = json!({
            "dependency": false,
            "head": "a".repeat(40),
            "files": [
                {"filename": "Cargo.toml", "additions": 2, "deletions": 1},
                {"filename": "src/lib.rs", "additions": 0, "deletions": 4}
            ],
            "checks": [
                {"name": "test", "state": "success", "url": "https://github.com/owner/repo/actions/runs/1"},
                {"name": "unsafe", "state": "failure", "url": "javascript:alert(1)"}
            ]
        });
        for (event, blockers, heading, action) in [
            (
                "APPROVE",
                vec![],
                "### Rady — Ready to merge",
                "use GitHub's Merge control when it is enabled",
            ),
            (
                "COMMENT",
                vec!["`test` is still running (pending)".to_owned()],
                "### Rady — Hold",
                "Wait for the named checks to finish",
            ),
            (
                "COMMENT",
                vec!["`test` needs attention (failure)".to_owned()],
                "### Rady — Hold",
                "Resolve the blockers below",
            ),
        ] {
            let body = render(&review, &context, event, &blockers, "<!-- marker -->");
            for expected in [
                heading,
                "Reviewed head: `aaaaaaaaaaaa` · CI snapshot: 1/2 checks successful",
                "What changed",
                "`Cargo.toml` · +2 −1",
                "`src/lib.rs` · +0 −4",
                "Checks",
                "[test](<https://github.com/owner/repo/actions/runs/1>) — success",
                "unsafe — failure",
                "Next action",
                action,
            ] {
                assert!(body.contains(expected), "missing {expected} in {body}");
            }
            assert!(!body.contains("javascript:"));
        }
    }

    #[test]
    fn repair_handoff_distinguishes_waiting_from_replacement_work() {
        let review = ModelReview {
            summary: "Reviewed. Safe handoff test".to_owned(),
            risk: Risk::Medium,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let context = json!({
            "number": 42, "url": "https://github.com/owner/repo/pull/42",
            "head": "a".repeat(40), "score": 96.0,
            "update_type": "version-update:semver-patch",
            "maintainer_changes": "false"
        });
        for (blockers, waiting) in [
            (
                vec!["`test` hasn't reported for this commit yet".to_owned()],
                true,
            ),
            (
                vec![
                    "Compatibility is unavailable; it needs manual review".to_owned(),
                    "Compatibility is unavailable; it needs manual review".to_owned(),
                ],
                false,
            ),
            (vec!["`test` needs attention (failure)".to_owned()], false),
        ] {
            let mut lines = Vec::new();
            repair_handoff(&mut lines, &review, &context, &blockers);
            let body = lines.join("\n");
            if waiting {
                assert!(body.is_empty());
            } else {
                for expected in [
                    "If a change is required, create a maintainer replacement PR; do not push to the Dependabot branch.",
                    r"- Source: \#42 https://github.com/owner/repo/pull/42",
                    "- Reviewed head: aaaa",
                    "- Compatibility: 96%",
                    "- Update type: version-update:semver-patch",
                    "- Maintainer changes: false",
                    "- Allowed files: approved manifests, lockfiles, Actions workflows and Dockerfiles only",
                    "- Required outcome: every configured and protected check passes on this commit",
                ] {
                    assert!(body.contains(expected), "missing {expected}");
                }
                if blockers
                    .iter()
                    .any(|blocker| blocker.contains("Compatibility is unavailable"))
                {
                    assert_eq!(body.matches("Compatibility is unavailable").count(), 1);
                }
            }
        }
    }

    #[test]
    fn percent_encoding_is_path_safe() {
        assert_eq!(percent_encode("feature/a b"), "feature%2Fa%20b");
    }

    #[test]
    fn dependency_paths_allow_manifests_and_reject_source() {
        for (paths, expected) in [
            (
                &[
                    "Cargo.toml",
                    "Cargo.lock",
                    "workspace/package.json",
                    "workspace/pnpm-lock.yaml",
                ][..],
                true,
            ),
            (
                &[
                    "api/requirements-dev.txt",
                    "api/constraints.txt",
                    "service/pom.xml",
                    "app/Package.resolved",
                ][..],
                true,
            ),
            (
                &[
                    "src/Directory.Packages.props",
                    "web/build.gradle.kts",
                    ".github/workflows/ci.yml",
                    "service/Dockerfile",
                ][..],
                true,
            ),
            (&[".github/dependabot.yml"][..], false),
            (&["src/lib.rs"][..], false),
            (&[".github/workflows/release.rs"][..], false),
            (&["setup.py"][..], false),
            (&["solution.sln"][..], false),
            (&["../Cargo.toml"][..], false),
        ] {
            let files = paths
                .iter()
                .map(|path| json!({"filename": path}))
                .collect::<Vec<_>>();
            assert_eq!(dependency_file_blockers(&files).is_empty(), expected);
        }
        assert!(
            !dependency_file_blockers(&[json!({
                "filename": "Cargo.toml", "previous_filename": "src/lib.rs"
            })])
            .is_empty()
        );
    }

    #[test]
    fn dependency_path_gate_does_not_change_regular_reviews() {
        let review = ModelReview {
            summary: "Reviewed".to_owned(),
            risk: Risk::Low,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let protection = json!({
            "enforce_admins": {"enabled": true},
            "required_status_checks": {"checks": [], "contexts": [], "strict": true}
        });
        let mut context = json!({
            "complete_diff": true, "checks": [], "dependency": false,
            "files": [{"filename": "src/lib.rs"}], "score": 95.0,
            "update_type": "version-update:semver-patch", "maintainer_changes": "false"
        });
        assert_eq!(
            decision(&review, &context, &[], Some(&protection))
                .unwrap()
                .0,
            "APPROVE"
        );

        context["dependency"] = json!(true);
        let (event, blockers) = decision(&review, &context, &[], Some(&protection)).unwrap();
        assert_eq!(event, "COMMENT");
        assert!(
            blockers
                .iter()
                .any(|blocker| blocker.contains("outside approved"))
        );
    }
}
