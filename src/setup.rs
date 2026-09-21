use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use serde_json::{Value, json};
use tempfile::NamedTempFile;

use crate::Result;
use crate::apps::{self, Identity};
use crate::github;

const TRUSTED_SOLVER_REPOSITORY: &str = "keys-i/rady";

#[derive(Clone, Debug)]
pub struct SourceRef {
    pub repository: String,
    pub commit: String,
}

impl SourceRef {
    pub fn resolve(value: Option<&str>) -> Result<Self> {
        match value {
            Some(value) => Self::parse(value),
            None => Self::latest(),
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        let (repository, commit) = value
            .split_once('@')
            .ok_or_else(|| anyhow!("--solver-ref requires keys-i/rady@40_LOWERCASE_COMMIT_SHA"))?;
        github::validate_repository(repository)?;
        if repository != TRUSTED_SOLVER_REPOSITORY || !valid_commit(commit) {
            bail!("--solver-ref requires keys-i/rady@40_LOWERCASE_COMMIT_SHA");
        }
        Ok(Self {
            repository: repository.to_owned(),
            commit: commit.to_owned(),
        })
    }

    fn latest() -> Result<Self> {
        let repository = github::api(
            &format!("repos/{TRUSTED_SOLVER_REPOSITORY}"),
            None,
            "GET",
            false,
        )?
        .ok_or_else(|| anyhow!("trusted solver repository response was empty"))?;
        let default_branch = default_branch(&repository)?;
        let branch = github::api(
            &format!(
                "repos/{TRUSTED_SOLVER_REPOSITORY}/branches/{}",
                percent_encode(default_branch)
            ),
            None,
            "GET",
            false,
        )?
        .ok_or_else(|| anyhow!("trusted solver branch response was empty"))?;
        Self::from_latest_response(default_branch, &branch)
    }

    fn from_latest_response(default_branch: &str, branch: &Value) -> Result<Self> {
        if branch["name"].as_str() != Some(default_branch) {
            bail!("GitHub returned a different trusted solver branch");
        }
        let commit = branch["commit"]["sha"]
            .as_str()
            .filter(|commit| valid_commit(commit))
            .ok_or_else(|| anyhow!("GitHub returned an invalid trusted solver commit"))?;
        Ok(Self {
            repository: TRUSTED_SOLVER_REPOSITORY.to_owned(),
            commit: commit.to_owned(),
        })
    }

    #[must_use]
    pub fn joined(&self) -> String {
        format!("{}@{}", self.repository, self.commit)
    }
}

fn default_branch(repository: &Value) -> Result<&str> {
    if repository["full_name"]
        .as_str()
        .is_none_or(|name| !name.eq_ignore_ascii_case(TRUSTED_SOLVER_REPOSITORY))
    {
        bail!("GitHub returned a different trusted solver repository");
    }
    repository["default_branch"]
        .as_str()
        .filter(|branch| {
            !branch.is_empty() && branch.len() <= 255 && !branch.chars().any(char::is_control)
        })
        .ok_or_else(|| anyhow!("trusted solver repository has no valid default branch"))
}

fn valid_commit(commit: &str) -> bool {
    commit.len() == 40
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub fn checks(values: &[String]) -> Result<Vec<String>> {
    if values.is_empty()
        || values.iter().any(|value| {
            value.trim().is_empty()
                || value.chars().any(char::is_control)
                || value.starts_with("Rady dependasolve")
        })
    {
        bail!("provide nonempty CI checks that do not name Rady dependasolve itself");
    }
    let mut seen = BTreeSet::new();
    Ok(values
        .iter()
        .filter(|value| seen.insert((*value).clone()))
        .cloned()
        .collect())
}

pub fn local_files(
    directory: &Path,
    source: &SourceRef,
    required: &[String],
    overwrite: bool,
) -> Result<BTreeMap<PathBuf, String>> {
    let root = directory
        .canonicalize()
        .context("--directory must be an existing directory")?;
    if !root.is_dir() {
        bail!("--directory must be a directory");
    }
    let workflow_ref = format!(
        "{}/.github/workflows/solve.yml@{}",
        source.repository, source.commit
    );
    let caller = include_str!("dependasolver/templates/dependency.solver.yml")
        .replace("__SOLVER_REF__", &workflow_ref)
        .replace("__SOURCE_REF__", &source.joined())
        .replace("__WORKFLOW_RUN_TRIGGER__", &workflow_run_trigger(&root)?)
        .replace(
            "__REQUIRED_CHECKS__",
            &serde_json::to_string(required)?.replace('\'', "''"),
        );
    let workflow = safe_path(&root, ".github/workflows/dependasolver.yml")?;
    let mut files = BTreeMap::from([(workflow.clone(), caller)]);
    let dependabot = [
        safe_path(&root, ".github/dependabot.yml")?,
        safe_path(&root, ".github/dependabot.yaml")?,
    ];
    if !dependabot.iter().any(|path| path.exists()) {
        files.insert(dependabot[0].clone(), dependabot_config(&root)?);
    }
    for (path, content) in &files {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.file_type().is_file()
                    || (fs::read_to_string(path)? != *content && (!overwrite || path != &workflow))
                {
                    bail!("refusing to overwrite existing content: {}", path.display());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(files)
}

fn workflow_run_trigger(root: &Path) -> Result<String> {
    let directory = root.join(".github/workflows");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };
    let mut names = BTreeSet::new();
    let mut count = 0;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let extension = path.extension().and_then(|value| value.to_str());
        if !entry.file_type()?.is_file()
            || !matches!(extension, Some("yml" | "yaml"))
            || matches!(
                path.file_name().and_then(|value| value.to_str()),
                Some("dependasolver.yml" | "dependasolver.yaml")
            )
        {
            continue;
        }
        count += 1;
        if count > 100 || entry.metadata()?.len() > 1_000_000 {
            bail!("GitHub Actions workflow discovery exceeded its safe limit");
        }
        let content = fs::read_to_string(&path)?;
        let name = content
            .lines()
            .find_map(|line| line.strip_prefix("name:").map(str::trim))
            .and_then(workflow_name)
            .unwrap_or_else(|| {
                format!(".github/workflows/{}", entry.file_name().to_string_lossy())
            });
        names.insert(name);
    }
    if names.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        "  workflow_run:\n    workflows: {}\n    types: [completed]\n    branches: ['dependabot/**']\n",
        serde_json::to_string(&names)?
    ))
}

fn workflow_name(raw: &str) -> Option<String> {
    let value = if raw.starts_with('"') && raw.ends_with('"') {
        serde_json::from_str(raw).ok()?
    } else if raw.starts_with('\'') && raw.ends_with('\'') {
        raw[1..raw.len().checked_sub(1)?].replace("''", "'")
    } else {
        raw.split_once(" #")
            .map_or(raw, |(value, _)| value)
            .trim()
            .to_owned()
    };
    (!value.is_empty() && value.len() <= 200 && !value.chars().any(char::is_control))
        .then_some(value)
}

const DEPENDABOT_ECOSYSTEMS: &[(&str, &[&str], &[&str])] = &[
    ("cargo", &["Cargo.toml"], &[]),
    ("npm", &["package.json"], &[]),
    (
        "pip",
        &["pyproject.toml", "requirements.txt", "Pipfile"],
        &[],
    ),
    ("bundler", &["Gemfile"], &[]),
    ("gomod", &["go.mod"], &[]),
    ("maven", &["pom.xml"], &[]),
    ("gradle", &["build.gradle", "build.gradle.kts"], &[]),
    ("composer", &["composer.json"], &[]),
    (
        "nuget",
        &["packages.config"],
        &["csproj", "fsproj", "vbproj"],
    ),
    ("docker", &["Dockerfile"], &[]),
];

fn dependabot_config(root: &Path) -> Result<String> {
    let directories = manifest_directories(root)?;
    let mut updates = dependabot_update("github-actions", "/");
    for (ecosystem, manifests, extensions) in DEPENDABOT_ECOSYSTEMS {
        for (directory, path) in &directories {
            if has_manifest(path, manifests, extensions)? {
                updates.push_str(&dependabot_update(ecosystem, directory));
            }
        }
    }
    Ok(format!("version: 2\nupdates:\n{updates}"))
}

fn manifest_directories(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut directories = BTreeMap::from([("/".to_owned(), root.to_path_buf())]);
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let path = entry.path();
            directories.insert(format!("/{}", entry.file_name().to_string_lossy()), path);
        }
    }
    Ok(directories)
}

fn has_manifest(directory: &Path, names: &[&str], extensions: &[&str]) -> Result<bool> {
    if names.iter().any(|name| directory.join(name).is_file()) {
        return Ok(true);
    }
    if extensions.is_empty() {
        return Ok(false);
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn dependabot_update(ecosystem: &str, directory: &str) -> String {
    format!(
        "  - package-ecosystem: {ecosystem}\n    directory: {directory:?}\n    schedule:\n      interval: weekly\n    open-pull-requests-limit: 3\n    groups:\n      {ecosystem}-minor-and-patch:\n        patterns: [\"*\"]\n        update-types: [minor, patch]\n"
    )
}

pub fn merged_checks(protection: Option<&Value>, required: &[String]) -> Vec<Value> {
    let current = protection
        .and_then(|value| value.get("required_status_checks"))
        .unwrap_or(&Value::Null);
    let mut result: Vec<Value> = current["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(json!({
                "context": item["context"].as_str()?,
                "app_id": item["app_id"].as_i64().unwrap_or(-1)
            }))
        })
        .collect();
    let mut present: BTreeSet<String> = result
        .iter()
        .filter_map(|item| item["context"].as_str().map(ToOwned::to_owned))
        .collect();
    let names = current["contexts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(required.iter().map(String::as_str));
    for name in names {
        if present.insert(name.to_owned()) {
            result.push(json!({"context": name, "app_id": -1}));
        }
    }
    result
}

pub fn protect(endpoint: &str, protection: Option<&Value>, required: &[String]) -> Result<()> {
    let status = json!({"strict": true, "checks": merged_checks(protection, required)});
    if protection.is_none() {
        github::api(
            endpoint,
            Some(&json!({
                "required_status_checks": status,
                "enforce_admins": true,
                "required_pull_request_reviews": Value::Null,
                "restrictions": Value::Null
            })),
            "PUT",
            false,
        )?;
    } else {
        github::api(
            &format!("{endpoint}/required_status_checks"),
            Some(&status),
            "PATCH",
            false,
        )?;
        github::api(&format!("{endpoint}/enforce_admins"), None, "POST", false)?;
    }
    Ok(())
}

fn write_setup_file(path: &Path, content: &[u8], overwrite: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("setup path has no parent directory"))?;
    fs::create_dir_all(parent)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                bail!("refusing to overwrite existing content: {}", path.display());
            }
            if fs::read(path)? == content {
                return Ok(());
            }
            if !overwrite {
                bail!("refusing to overwrite existing content: {}", path.display());
            }
            let mut temporary = NamedTempFile::new_in(parent)?;
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
            temporary.write_all(content)?;
            temporary.as_file().sync_all()?;
            temporary
                .persist(path)
                .map_err(|error| error.error)
                .with_context(|| format!("could not replace setup file {}", path.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
            file.write_all(content)?;
            file.sync_all()?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn install(
    repo: &str,
    source: &SourceRef,
    required: &[String],
    directory: &Path,
    new_app: bool,
    identity: Identity,
    overwrite: bool,
) -> Result<()> {
    let prefix = identity.prefix();
    let client_name = format!("{prefix}_APP_CLIENT_ID");
    let key_name = format!("{prefix}_APP_PRIVATE_KEY");
    let slug_name = format!("{prefix}_APP_SLUG");
    let files = local_files(directory, source, required, overwrite)?;
    let info = github::api(&format!("repos/{repo}"), None, "GET", false)?
        .ok_or_else(|| anyhow!("repository response was empty"))?;
    if info["full_name"]
        .as_str()
        .is_none_or(|name| !name.eq_ignore_ascii_case(repo))
        || info["permissions"]["admin"].as_bool() != Some(true)
    {
        bail!("the target repository requires administration access");
    }
    if info["private"].as_bool().is_none() {
        bail!("repository response omitted visibility");
    }
    if !matches!(
        info["owner"]["type"].as_str(),
        Some("User" | "Organization")
    ) {
        bail!("only personal and organisation repositories are supported");
    }
    let source_file = github::api(
        &format!(
            "repos/{}/contents/.github/workflows/solve.yml?ref={}",
            source.repository, source.commit
        ),
        None,
        "GET",
        false,
    )?
    .ok_or_else(|| anyhow!("source workflow response was empty"))?;
    if source_file["type"] != "file" {
        bail!("publish the source workflow at the immutable commit first");
    }
    for (path, content) in files {
        let replace = overwrite && path.ends_with(".github/workflows/dependasolver.yml");
        write_setup_file(&path, content.as_bytes(), replace)?;
    }
    let branch = percent_encode(info["default_branch"].as_str().unwrap_or_default());
    let endpoint = format!("repos/{repo}/branches/{branch}/protection");
    let key = if new_app {
        None
    } else {
        github::api(
            &format!("repos/{repo}/actions/secrets/{key_name}"),
            None,
            "GET",
            true,
        )?
    };
    let client = if new_app {
        None
    } else {
        github::api(
            &format!("repos/{repo}/actions/variables/{client_name}"),
            None,
            "GET",
            true,
        )?
    };
    if key.is_none() != client.is_none() {
        bail!("incomplete App setup: configure both credentials or use --new-app --apply");
    }
    if key.is_none() {
        let app = apps::register_app(repo, identity)?;
        apps::credentials(repo, &app, identity)?;
    } else {
        let slug = github::api(
            &format!("repos/{repo}/actions/variables/{slug_name}"),
            None,
            "GET",
            true,
        )?
        .and_then(|value| value["value"].as_str().map(ToOwned::to_owned))
        .unwrap_or_default();
        let app = apps::public_app(&slug)?;
        apps::require_app_owner(&app)?;
        apps::require_permissions(&app)?;
        if &app["client_id"]
            != client
                .as_ref()
                .map_or(&Value::Null, |value| &value["value"])
        {
            bail!("existing App slug and Client ID do not match; no credentials were changed");
        }
    }
    let protection = github::api(&endpoint, None, "GET", true)?;
    protect(&endpoint, protection.as_ref(), required)?;
    github::api(
        &format!("repos/{repo}"),
        Some(&json!({"allow_auto_merge": true, "allow_squash_merge": true})),
        "PATCH",
        false,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    repo: &str,
    source: &SourceRef,
    required: &[String],
    directory: &Path,
    identity: Identity,
    new_app: bool,
    overwrite: bool,
    apply: bool,
) -> Result<Value> {
    github::validate_repository(repo)?;
    let required = checks(required)?;
    let files = local_files(directory, source, &required, overwrite)?;
    let preview = json!({
        "repository": repo,
        "source": source.joined(),
        "required_checks": required,
        "files": files.keys().map(|path| path.display().to_string()).collect::<Vec<_>>(),
        "app_owner": apps::APP_OWNER,
        "app_public": true,
        "new_app": new_app,
        "app_permissions": apps::permissions(),
        "identity": identity.slug(),
        "overwrite": overwrite,
        "apply": apply,
    });
    if apply {
        install(
            repo, source, &required, directory, new_app, identity, overwrite,
        )?;
    }
    Ok(preview)
}

fn safe_path(root: &Path, name: &str) -> Result<PathBuf> {
    let path = root.join(name);
    let parent = path.parent().ok_or_else(|| anyhow!("invalid local path"))?;
    let resolved_parent = nearest_existing(parent)?.canonicalize()?;
    if !resolved_parent.starts_with(root) {
        bail!("refusing a path outside --directory: {name}");
    }
    Ok(path)
}

fn nearest_existing(path: &Path) -> Result<&Path> {
    let mut current = path;
    while !current.exists() {
        current = current
            .parent()
            .ok_or_else(|| anyhow!("path has no existing parent"))?;
    }
    Ok(current)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_reference_table_covers_repository_and_commit_edges() {
        for (value, valid) in [
            (format!("keys-i/rady@{}", "a".repeat(40)), true),
            (format!("owner/repo@{}", "a".repeat(40)), false),
            (format!("keys-i/rady@{}", "A".repeat(40)), false),
            ("keys-i/rady@short".to_owned(), false),
            (format!("keys-i/rady@{}", "g".repeat(40)), false),
        ] {
            assert_eq!(SourceRef::parse(&value).is_ok(), valid, "{value}");
        }
    }

    #[test]
    fn latest_source_response_is_strict_and_immutable() -> Result<()> {
        let commit = "a".repeat(40);
        let source = SourceRef::from_latest_response(
            "main",
            &json!({"name": "main", "commit": {"sha": commit}}),
        )?;
        assert_eq!(source.joined(), format!("keys-i/rady@{commit}"));
        for (default_branch, response) in [
            ("main", json!({"name": "other", "commit": {"sha": commit}})),
            ("main", json!({"name": "main", "commit": {"sha": "short"}})),
            (
                "main",
                json!({"name": "main", "commit": {"sha": "A".repeat(40)}}),
            ),
        ] {
            assert!(
                SourceRef::from_latest_response(default_branch, &response).is_err(),
                "{response}"
            );
        }
        for (response, valid) in [
            (
                json!({"full_name": "keys-i/rady", "default_branch": "main"}),
                true,
            ),
            (
                json!({"full_name": "KEYS-I/RADY", "default_branch": "feature/a"}),
                true,
            ),
            (
                json!({"full_name": "other/rady", "default_branch": "main"}),
                false,
            ),
            (
                json!({"full_name": "keys-i/rady", "default_branch": ""}),
                false,
            ),
            (
                json!({"full_name": "keys-i/rady", "default_branch": "bad\nbranch"}),
                false,
            ),
            (json!({}), false),
        ] {
            assert_eq!(default_branch(&response).is_ok(), valid, "{response}");
        }
        Ok(())
    }

    #[test]
    fn generated_workflow_overwrites_by_default_and_can_be_protected() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let workflow = temporary.path().join(".github/workflows/dependasolver.yml");
        fs::create_dir_all(workflow.parent().expect("workflow parent"))?;
        fs::write(&workflow, "existing workflow\n")?;
        let source = SourceRef::parse(&format!("keys-i/rady@{}", "b".repeat(40)))?;

        assert!(local_files(temporary.path(), &source, &["test".into()], false).is_err());
        let files = local_files(temporary.path(), &source, &["test".into()], true)?;
        let (workflow, generated) = files
            .iter()
            .find(|(path, _)| path.ends_with(".github/workflows/dependasolver.yml"))
            .expect("generated workflow");
        write_setup_file(workflow, generated.as_bytes(), true)?;
        assert_eq!(fs::read_to_string(workflow)?, *generated);

        assert!(write_setup_file(workflow, b"blocked update\n", false).is_err());
        assert_eq!(fs::read_to_string(workflow)?, *generated);
        Ok(())
    }

    #[test]
    fn merged_checks_deduplicate_contexts_and_preserve_bindings() {
        let protection = json!({"required_status_checks": {"contexts": ["test"], "checks": [{"context": "lint", "app_id": 4}]}});
        let merged = merged_checks(Some(&protection), &["test".to_owned(), "audit".to_owned()]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0]["app_id"], 4);
    }

    #[test]
    fn setup_text_validation_is_table_driven() -> Result<()> {
        assert_eq!(
            checks(&["test".into(), "lint".into(), "test".into()])?,
            ["test", "lint"]
        );
        assert!(checks(&["Rady dependasolve gate".into()]).is_err());
        for (raw, expected) in [
            ("CI", Some("CI")),
            ("CI # branch checks", Some("CI")),
            ("\"Build\"", Some("Build")),
            ("'Release ''safe'''", Some("Release 'safe'")),
            ("", None),
            ("bad\nname", None),
        ] {
            assert_eq!(workflow_name(raw).as_deref(), expected, "{raw:?}");
        }
        Ok(())
    }

    #[test]
    fn generated_dependabot_config_covers_root_and_workspace_manifests() -> Result<()> {
        for (manifest, ecosystem, directory) in [
            ("Cargo.toml", "cargo", "/"),
            ("package.json", "npm", "/"),
            ("requirements.txt", "pip", "/"),
            ("Gemfile", "bundler", "/"),
            ("go.mod", "gomod", "/"),
            ("pom.xml", "maven", "/"),
            ("build.gradle.kts", "gradle", "/"),
            ("composer.json", "composer", "/"),
            ("project.csproj", "nuget", "/"),
            ("Dockerfile", "docker", "/"),
            ("web/package.json", "npm", "/web"),
        ] {
            let temporary = tempfile::tempdir()?;
            let path = temporary.path().join(manifest);
            fs::create_dir_all(path.parent().expect("manifest parent"))?;
            fs::write(path, "")?;
            let config = dependabot_config(temporary.path())?;
            assert!(
                config.contains(&format!(
                    "package-ecosystem: {ecosystem}\n    directory: {directory:?}"
                )),
                "{manifest}"
            );
            assert!(config.contains(&format!("{ecosystem}-minor-and-patch:")));
            assert!(config.contains("package-ecosystem: github-actions\n    directory: \"/\""));
        }
        for (manifest, ecosystem) in [("setup.py", "pip"), ("solution.sln", "nuget")] {
            let temporary = tempfile::tempdir()?;
            fs::write(temporary.path().join(manifest), "")?;
            assert!(
                !dependabot_config(temporary.path())?
                    .contains(&format!("package-ecosystem: {ecosystem}")),
                "{manifest} must not opt into executable or solution-file updates"
            );
        }
        Ok(())
    }

    #[test]
    fn existing_dependabot_configuration_is_left_untouched() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        fs::create_dir_all(temporary.path().join(".github"))?;
        fs::write(
            temporary.path().join(".github/dependabot.yaml"),
            "version: 2\n",
        )?;
        fs::create_dir_all(temporary.path().join(".github/workflows"))?;
        fs::write(
            temporary.path().join(".github/workflows/ci.yml"),
            "name: CI\non: [pull_request]\n",
        )?;
        let source = SourceRef::parse(&format!("keys-i/rady@{}", "a".repeat(40)))?;
        let files = local_files(temporary.path(), &source, &["check".to_owned()], true)?;
        let workflow = files
            .iter()
            .find(|(path, _)| path.ends_with(".github/workflows/dependasolver.yml"))
            .map(|(_, content)| content)
            .expect("generated caller workflow");
        assert!(workflow.contains("check_run:"));
        assert!(workflow.contains("workflow_run:"));
        assert!(workflow.contains("workflows: [\"CI\"]"));
        assert!(workflow.contains("name: Rady dependasolve gate"));
        let solver = include_str!("../.github/workflows/solve.yml");
        let suspend = solver
            .find("Suspend stale Dependabot auto-merge")
            .expect("early auto-merge suspension");
        let checkout = solver.find("actions/checkout@").expect("solver checkout");
        let preflight = solver
            .find("Validate pull request trust boundary")
            .expect("public repository preflight");
        let harness_gate = solver
            .find("Require a native public review harness")
            .expect("public harness gate");
        let review = solver.find("\n  review:\n").expect("review job");
        assert!(preflight < checkout);
        assert!(preflight < review);
        assert!(harness_gate < checkout);
        assert!(suspend < checkout);
        assert!(solver.contains("runs-on: ubuntu-latest"));
        assert!(solver.contains(
            "  review:\n    needs: preflight\n    if: needs.preflight.outputs.allowed == 'true'"
        ));
        assert!(
            solver.contains(
                "Public repositories run Rady only for same-repository Dependabot updates"
            )
        );
        assert!(solver.contains(".user.login == \"dependabot[bot]\""));
        assert!(solver.contains(".head.repo.full_name == $repo"));
        assert_eq!(solver.matches("actions/checkout@").count(), 1);
        assert!(solver.contains("repository: ${{ steps.source.outputs.repository }}"));
        assert!(solver.contains("^keys-i/rady@([a-f0-9]{40})$"));
        assert!(solver.contains("repository=keys-i/rady"));
        assert!(
            solver
                .find("Validate trusted solver source")
                .expect("source gate")
                < checkout
        );
        assert!(
            !files
                .keys()
                .any(|path| path.ends_with(".github/dependabot.yml"))
        );
        Ok(())
    }
}
