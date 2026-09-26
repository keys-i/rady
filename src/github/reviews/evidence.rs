use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

use crate::Result;
use crate::github::GitHub;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependabotMetadata {
    pub update_type: String,
    pub maintainer_changes: String,
}

pub fn resolve(github: &GitHub, number: u64) -> Result<(Value, bool, Option<DependabotMetadata>)> {
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
    let metadata = dependency
        .then(|| dependabot_metadata(github, number, &pull))
        .transpose()?;
    Ok((pull, dependency, metadata))
}

fn dependabot_metadata(github: &GitHub, number: u64, pull: &Value) -> Result<DependabotMetadata> {
    let expected_head = text(pull, &["head", "sha"])?;
    let commits = github.pages(&format!("pulls/{number}/commits"), None)?;
    Ok(metadata_from_commits(&commits, expected_head)
        .unwrap_or_else(DependabotMetadata::unsupported))
}

impl DependabotMetadata {
    fn unsupported() -> Self {
        Self {
            update_type: "unsupported".to_owned(),
            maintainer_changes: "unknown".to_owned(),
        }
    }
}

fn metadata_from_commits(commits: &[Value], expected_head: &str) -> Option<DependabotMetadata> {
    let [commit] = commits else {
        return None;
    };
    if commit["sha"] != expected_head {
        return None;
    }
    if commit["author"]["login"] != "dependabot[bot]"
        || commit["committer"]["login"] != "dependabot[bot]"
        || commit["commit"]["verification"]["verified"] != true
    {
        return None;
    }
    let update_type = classify_dependabot_subject(commit["commit"]["message"].as_str()?)?;
    Some(DependabotMetadata {
        update_type,
        maintainer_changes: "false".to_owned(),
    })
}

fn classify_dependabot_subject(message: &str) -> Option<String> {
    let subject = message.lines().next()?.strip_prefix("Bump ")?;
    let (package, versions) = subject.rsplit_once(" from ")?;
    let (from, to) = versions.split_once(" to ")?;
    if package.is_empty()
        || from.is_empty()
        || to.is_empty()
        || package.chars().any(char::is_whitespace)
        || from.chars().any(char::is_whitespace)
        || to.chars().any(char::is_whitespace)
    {
        return None;
    }
    let from = numeric_version(from)?;
    let to = numeric_version(to)?;
    if to <= from {
        return None;
    }
    let kind = if to[0] != from[0] {
        "major"
    } else if to[1] != from[1] {
        "minor"
    } else {
        "patch"
    };
    Some(format!("version-update:semver-{kind}"))
}

fn numeric_version(value: &str) -> Option<[u64; 3]> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts = value.split('.').collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len()) {
        return None;
    }
    let mut version = [0; 3];
    for (index, part) in parts.into_iter().enumerate() {
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return None;
        }
        version[index] = part.parse().ok()?;
    }
    Some(version)
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
    fn table_driven_dependabot_subject_classification_is_fail_closed() {
        for (subject, expected) in [
            (
                "Bump actions/checkout from v4.1.7 to v4.2.2",
                Some("version-update:semver-minor"),
            ),
            (
                "Bump serde from 1.0.0 to 1.0.1",
                Some("version-update:semver-patch"),
            ),
            (
                "Bump rust from 1.85 to 2.0",
                Some("version-update:semver-major"),
            ),
            ("Bump serde from 1.0.0-beta.1 to 1.0.0", None),
            ("Bump image digest from abc to def", None),
            ("Bump serde from 1.0.1 to 1.0.0", None),
            (
                "Bump serde from 1.0.0 to 1.0.1\n\nDependabot command: @dependabot rebase",
                Some("version-update:semver-patch"),
            ),
            ("Bump serde from 1.0.0 to 1.0.1 and rand", None),
            ("Bump serde and rand from 1.0.0 to 1.0.1", None),
        ] {
            assert_eq!(
                classify_dependabot_subject(subject).as_deref(),
                expected,
                "{subject}"
            );
        }
    }

    #[test]
    fn dependabot_metadata_is_trusted_only_for_one_verified_bot_bump() {
        let commit = json!({
            "sha": "a".repeat(40),
            "author": {"login": "dependabot[bot]"},
            "committer": {"login": "dependabot[bot]"},
            "commit": {
                "message": "Bump serde from 1.0.0 to 1.0.1\n\nDependabot command: @dependabot rebase",
                "verification": {"verified": true}
            }
        });
        for (commits, head, update_type, maintainer_changes) in [
            (
                vec![commit.clone()],
                "a".repeat(40),
                "version-update:semver-patch",
                "false",
            ),
            (
                vec![json!({
                    "sha": "a".repeat(40),
                    "author": {"login": "dependabot[bot]"},
                    "committer": {"login": "web-flow"},
                    "commit": {"message": "Bump serde from 1.0.0 to 1.0.1", "verification": {"verified": true}}
                })],
                "a".repeat(40),
                "unsupported",
                "unknown",
            ),
            (
                vec![commit.clone()],
                "b".repeat(40),
                "unsupported",
                "unknown",
            ),
            (
                vec![commit.clone(), commit.clone()],
                "a".repeat(40),
                "unsupported",
                "unknown",
            ),
            (
                vec![json!({
                    "sha": "a".repeat(40),
                    "author": {"login": "dependabot[bot]"},
                    "committer": {"login": "dependabot[bot]"},
                    "commit": {"message": "Bump serde from 1.0.0 to 1.0.1", "verification": {"verified": false}}
                })],
                "a".repeat(40),
                "unsupported",
                "unknown",
            ),
            (
                vec![json!({
                    "sha": "a".repeat(40),
                    "author": {"login": "dependabot[bot]"},
                    "committer": {"login": "someone-else"},
                    "commit": {"message": "Bump serde from 1.0.0 to 1.0.1", "verification": {"verified": true}}
                })],
                "a".repeat(40),
                "unsupported",
                "unknown",
            ),
        ] {
            let metadata = metadata_from_commits(&commits, &head)
                .unwrap_or_else(DependabotMetadata::unsupported);
            assert_eq!(metadata.update_type, update_type);
            assert_eq!(metadata.maintainer_changes, maintainer_changes);
        }
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
}
