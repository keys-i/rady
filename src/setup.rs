use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use serde_json::{Value, json};
use tempfile::NamedTempFile;

use crate::Result;
use crate::apps::{self, Identity};
use crate::github;

const TRUSTED_SOLVER_REPOSITORY: &str = "keys-i/rady";
const TERMS_VERSION: &str = "2026-09-23";
const PRIVACY_VERSION: &str = "2026-09-23";
const TERMS_URL: &str = "https://github.com/keys-i/rady/blob/main/docs/TERMS.md";
const PRIVACY_URL: &str = "https://github.com/keys-i/rady/blob/main/docs/PRIVACY.md";
const CONSENT_ISSUE_TITLE: &str = "Rady service agreement";

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
    _identity: Identity,
    overwrite: bool,
) -> Result<BTreeMap<PathBuf, String>> {
    setup_files(directory, source, required, overwrite, None)
}

fn setup_files(
    directory: &Path,
    source: &SourceRef,
    required: &[String],
    overwrite: bool,
    agreement: Option<&Value>,
) -> Result<BTreeMap<PathBuf, String>> {
    let root = directory
        .canonicalize()
        .context("--directory must be an existing directory")?;
    if !root.is_dir() {
        bail!("--directory must be a directory");
    }
    let configuration = format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "schema": 1,
            "source": source.joined(),
            "checks": required,
            "agreement": agreement.cloned().unwrap_or(Value::Null),
        }))?
    );
    let config = safe_path(&root, ".github/rady.json")?;
    let mut files = BTreeMap::from([(config.clone(), configuration)]);
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
                    || (fs::read_to_string(path)? != *content && (!overwrite || path != &config))
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
    accept_terms: bool,
) -> Result<()> {
    if !accept_terms {
        bail!("read {TERMS_URL} and {PRIVACY_URL}, then rerun with --accept-terms if you agree");
    }
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
    if !repo
        .split_once('/')
        .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(apps::APP_OWNER))
    {
        bail!(
            "central GitHub Actions orchestration currently supports repositories owned by {}",
            apps::APP_OWNER
        );
    }
    for workflow in ["orchestrate.yml", "solve.yml"] {
        let source_file = github::api(
            &format!(
                "repos/{}/contents/.github/workflows/{workflow}?ref={}",
                source.repository, source.commit
            ),
            None,
            "GET",
            false,
        )?
        .ok_or_else(|| anyhow!("source workflow response was empty"))?;
        if source_file["type"] != "file" {
            bail!("publish every source workflow at the immutable commit first");
        }
    }
    let existing = if new_app {
        None
    } else {
        existing_app(TRUSTED_SOLVER_REPOSITORY, identity)?
    };
    let effective_identity = if let Some(existing) = existing {
        let effective_identity = existing.identity;
        verify_existing_app(existing)?;
        effective_identity
    } else if new_app {
        let app = apps::register_app(TRUSTED_SOLVER_REPOSITORY, identity)?;
        apps::credentials(TRUSTED_SOLVER_REPOSITORY, &app, identity)?;
        identity
    } else {
        bail!(
            "no existing {} App credentials; use --new-app --apply to register one",
            identity.display()
        );
    };
    let existing = existing_app(TRUSTED_SOLVER_REPOSITORY, effective_identity)?
        .ok_or_else(|| anyhow!("central App credentials disappeared during setup"))?;
    verify_existing_app(existing.clone())?;
    ensure_installation(repo, &existing.slug)?;
    let agreement = agreement(repo)?;
    let files = setup_files(directory, source, required, overwrite, Some(&agreement))?;
    for (path, content) in files {
        let replace = overwrite && path.ends_with(".github/rady.json");
        write_setup_file(&path, content.as_bytes(), replace)?;
    }
    github::api(
        &format!("repos/{repo}"),
        Some(&json!({"allow_auto_merge": true, "allow_squash_merge": true})),
        "PATCH",
        false,
    )?;
    Ok(())
}

fn agreement(repo: &str) -> Result<Value> {
    let user = github::api("user", None, "GET", false)?
        .ok_or_else(|| anyhow!("GitHub returned no authenticated user"))?;
    let accepted_by = user["login"]
        .as_str()
        .filter(|login| {
            !login.is_empty()
                && login.len() <= 100
                && login
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .ok_or_else(|| anyhow!("GitHub returned an invalid authenticated user"))?;
    let accepted_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    let receipt = create_consent_receipt(repo, accepted_by)?;
    Ok(json!({
        "terms": TERMS_VERSION,
        "privacy": PRIVACY_VERSION,
        "accepted_by": accepted_by,
        "accepted_at_unix": accepted_at,
        "issue": receipt.issue,
        "comment": receipt.comment,
    }))
}

#[derive(Clone, Copy, Debug)]
struct ConsentReceipt {
    issue: u64,
    comment: u64,
}

fn consent_comment(repo: &str) -> String {
    format!(
        "Rady service agreement acceptance\n\nI accept the Rady Terms of Use ({TERMS_VERSION}) and Privacy Policy ({PRIVACY_VERSION}) for {repo}."
    )
}

fn create_consent_receipt(repo: &str, accepted_by: &str) -> Result<ConsentReceipt> {
    let issue = github::api(
        &format!("repos/{repo}/issues"),
        Some(&json!({
            "title": CONSENT_ISSUE_TITLE,
            "body": format!("Accepted by @{accepted_by}. This closed issue is Rady's service-agreement receipt."),
        })),
        "POST",
        false,
    )?
    .ok_or_else(|| anyhow!("GitHub returned no consent issue"))?;
    let number = issue["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or_else(|| anyhow!("GitHub returned an invalid consent issue"))?;
    let comment = github::api(
        &format!("repos/{repo}/issues/{number}/comments"),
        Some(&json!({"body": consent_comment(repo)})),
        "POST",
        false,
    )?
    .ok_or_else(|| anyhow!("GitHub returned no consent comment"))?;
    let comment = comment["id"]
        .as_u64()
        .filter(|comment| *comment > 0)
        .ok_or_else(|| anyhow!("GitHub returned an invalid consent comment"))?;
    github::api(
        &format!("repos/{repo}/issues/{number}"),
        Some(&json!({"state": "closed"})),
        "PATCH",
        false,
    )?;
    Ok(ConsentReceipt {
        issue: number,
        comment,
    })
}

pub(crate) fn accepted_configuration(value: &Value) -> bool {
    value["schema"].as_u64() == Some(1)
        && value["agreement"]["terms"].as_str() == Some(TERMS_VERSION)
        && value["agreement"]["privacy"].as_str() == Some(PRIVACY_VERSION)
        && value["agreement"]["accepted_by"]
            .as_str()
            .is_some_and(|login| {
                !login.is_empty()
                    && login.len() <= 100
                    && login
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        && value["agreement"]["accepted_at_unix"]
            .as_u64()
            .is_some_and(|time| time > 0)
        && value["agreement"]["issue"]
            .as_u64()
            .is_some_and(|number| number > 0)
        && value["agreement"]["comment"]
            .as_u64()
            .is_some_and(|number| number > 0)
}

pub(crate) fn verified_configuration(github: &github::GitHub, value: &Value) -> Result<bool> {
    if !accepted_configuration(value) {
        return Ok(false);
    }
    let agreement = &value["agreement"];
    let accepted_by = agreement["accepted_by"]
        .as_str()
        .ok_or_else(|| anyhow!("accepted configuration omitted its signer"))?;
    let issue = agreement["issue"]
        .as_u64()
        .ok_or_else(|| anyhow!("accepted configuration omitted its receipt issue"))?;
    let comment = agreement["comment"]
        .as_u64()
        .ok_or_else(|| anyhow!("accepted configuration omitted its receipt comment"))?;
    let issue_value = github.api_optional(&format!("issues/{issue}"), None, "GET")?;
    let comment_value = github.api_optional(&format!("issues/comments/{comment}"), None, "GET")?;
    let permission = github.api_optional(
        &format!("collaborators/{accepted_by}/permission"),
        None,
        "GET",
    )?;
    Ok(receipt_matches(
        github.repo(),
        accepted_by,
        issue,
        comment,
        issue_value.as_ref(),
        comment_value.as_ref(),
        permission.as_ref(),
    ))
}

fn receipt_matches(
    repo: &str,
    accepted_by: &str,
    issue: u64,
    comment: u64,
    issue_value: Option<&Value>,
    comment_value: Option<&Value>,
    permission: Option<&Value>,
) -> bool {
    let issue_url = format!("https://github.com/{repo}/issues/{issue}");
    let comment_issue_url = format!("https://api.github.com/repos/{repo}/issues/{issue}");
    issue_value.is_some_and(|value| {
        value["number"].as_u64() == Some(issue)
            && value["html_url"].as_str() == Some(&issue_url)
            && value["title"].as_str() == Some(CONSENT_ISSUE_TITLE)
            && value["state"].as_str() == Some("closed")
            && value.get("pull_request").is_none()
    }) && comment_value.is_some_and(|value| {
        value["id"].as_u64() == Some(comment)
            && value["issue_url"].as_str() == Some(&comment_issue_url)
            && value["user"]["login"].as_str() == Some(accepted_by)
            && value["body"].as_str() == Some(&consent_comment(repo))
    }) && permission.is_some_and(|value| value["permission"].as_str() == Some("admin"))
}

#[derive(Clone)]
struct ExistingApp {
    identity: Identity,
    client_id: String,
    slug: String,
}

fn existing_app(repo: &str, identity: Identity) -> Result<Option<ExistingApp>> {
    let primary = credentials(repo, identity)?;
    if identity == Identity::Dependasolver || primary.is_some() {
        return Ok(primary);
    }
    credentials(repo, Identity::Dependasolver)
}

fn credentials(repo: &str, identity: Identity) -> Result<Option<ExistingApp>> {
    let prefix = identity.prefix();
    let key = github::api(
        &format!("repos/{repo}/actions/secrets/{prefix}_APP_PRIVATE_KEY"),
        None,
        "GET",
        true,
    )?;
    let client = github::api(
        &format!("repos/{repo}/actions/variables/{prefix}_APP_CLIENT_ID"),
        None,
        "GET",
        true,
    )?;
    let slug = github::api(
        &format!("repos/{repo}/actions/variables/{prefix}_APP_SLUG"),
        None,
        "GET",
        true,
    )?;
    match (key, client, slug) {
        (None, None, None) => Ok(None),
        (Some(_), Some(client), Some(slug)) => {
            let client_id = client["value"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("incomplete {prefix} App setup: client ID is empty"))?;
            let slug = slug["value"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("incomplete {prefix} App setup: slug is empty"))?;
            Ok(Some(ExistingApp {
                identity,
                client_id: client_id.to_owned(),
                slug: slug.to_owned(),
            }))
        }
        _ => bail!(
            "incomplete {prefix} App setup: configure all App credentials or use --new-app --apply"
        ),
    }
}

fn verify_existing_app(existing: ExistingApp) -> Result<()> {
    let app = apps::public_app(&existing.slug)?;
    apps::require_app_owner(&app)?;
    apps::require_permissions(&app)?;
    if app["client_id"].as_str() != Some(&existing.client_id) {
        bail!(
            "existing {} App slug and Client ID do not match; no credentials were changed",
            existing.identity.display()
        );
    }
    Ok(())
}

fn ensure_installation(repo: &str, slug: &str) -> Result<()> {
    if installation_matches(repo, slug)? {
        return Ok(());
    }
    apps::open_installation(slug, repo)?;
    eprintln!("Press Enter after granting radyybot access to {repo}");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if !installation_matches(repo, slug)? {
        bail!("radyybot is not installed for {repo}");
    }
    Ok(())
}

fn installation_matches(repo: &str, slug: &str) -> Result<bool> {
    let installation = github::api(&format!("repos/{repo}/installation"), None, "GET", true)?;
    Ok(installation.is_some_and(|installation| {
        installation["app_slug"]
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(slug))
    }))
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
    accept_terms: bool,
) -> Result<Value> {
    github::validate_repository(repo)?;
    let required = checks(required)?;
    let files = local_files(directory, source, &required, identity, overwrite)?;
    let preview = json!({
        "repository": repo,
        "source": source.joined(),
        "required_checks": required,
        "files": files.keys().map(|path| path.display().to_string()).collect::<Vec<_>>(),
        "app_owner": apps::APP_OWNER,
        "credentials_repository": TRUSTED_SOLVER_REPOSITORY,
        "orchestration": "central",
        "terms": {"version": TERMS_VERSION, "url": TERMS_URL},
        "privacy": {"version": PRIVACY_VERSION, "url": PRIVACY_URL},
        "agreement_required": !accept_terms,
        "app_public": true,
        "new_app": new_app,
        "app_permissions": apps::permissions(),
        "identity": identity.slug(),
        "overwrite": overwrite,
        "apply": apply,
    });
    if apply {
        install(
            repo,
            source,
            &required,
            directory,
            new_app,
            identity,
            overwrite,
            accept_terms,
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
    fn generated_configuration_overwrites_by_default_and_can_be_protected() -> Result<()> {
        let source = SourceRef::parse(&format!("keys-i/rady@{}", "b".repeat(40)))?;
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join(".github/rady.json");
        fs::create_dir_all(path.parent().expect("generated file parent"))?;
        fs::write(&path, "existing generated content\n")?;
        assert!(
            local_files(
                temporary.path(),
                &source,
                &["test".into()],
                Identity::Rady,
                false
            )
            .is_err(),
            "--no-overwrite must protect the agreement file"
        );
        let files = local_files(
            temporary.path(),
            &source,
            &["test".into()],
            Identity::Rady,
            true,
        )?;
        let generated = files
            .iter()
            .find(|(candidate, _)| candidate.ends_with(".github/rady.json"))
            .map(|(_, content)| content)
            .expect("generated configuration");
        write_setup_file(&path, generated.as_bytes(), true)?;
        assert_eq!(fs::read_to_string(&path)?, *generated);
        assert!(write_setup_file(&path, b"blocked update\n", false).is_err());
        assert_eq!(fs::read_to_string(&path)?, *generated);
        Ok(())
    }

    #[test]
    fn setup_text_validation_is_table_driven() -> Result<()> {
        assert_eq!(
            checks(&["test".into(), "lint".into(), "test".into()])?,
            ["test", "lint"]
        );
        assert!(checks(&["Rady dependasolve gate".into()]).is_err());
        for (configuration, accepted) in [
            (
                json!({
                    "schema": 1,
                    "agreement": {
                        "terms": TERMS_VERSION,
                        "privacy": PRIVACY_VERSION,
                        "accepted_by": "keys-i",
                        "accepted_at_unix": 1,
                        "issue": 1,
                        "comment": 2,
                    }
                }),
                true,
            ),
            (json!({"schema": 1, "agreement": null}), false),
            (
                json!({
                    "schema": 1,
                    "agreement": {
                        "terms": TERMS_VERSION,
                        "privacy": PRIVACY_VERSION,
                        "accepted_by": "keys-i",
                        "accepted_at_unix": 1,
                    }
                }),
                false,
            ),
            (
                json!({
                    "schema": 1,
                    "agreement": {
                        "terms": "old",
                        "privacy": PRIVACY_VERSION,
                        "accepted_by": "keys-i",
                        "accepted_at_unix": 1,
                        "issue": 1,
                        "comment": 2,
                    }
                }),
                false,
            ),
        ] {
            assert_eq!(accepted_configuration(&configuration), accepted);
        }
        Ok(())
    }

    #[test]
    fn consent_receipt_requires_exact_authenticated_evidence() {
        let repo = "keys-i/rady";
        let issue = json!({
            "number": 7,
            "html_url": "https://github.com/keys-i/rady/issues/7",
            "title": CONSENT_ISSUE_TITLE,
            "state": "closed",
        });
        let comment = json!({
            "id": 9,
            "issue_url": "https://api.github.com/repos/keys-i/rady/issues/7",
            "user": {"login": "keys-i"},
            "body": consent_comment(repo),
        });
        let permission = json!({"permission": "admin"});
        for (issue_value, comment_value, permission_value, valid) in [
            (issue.clone(), comment.clone(), permission.clone(), true),
            (
                json!({"state": "open"}),
                comment.clone(),
                permission.clone(),
                false,
            ),
            (
                issue.clone(),
                json!({"user": {"login": "other"}}),
                permission.clone(),
                false,
            ),
            (issue, comment, json!({"permission": "write"}), false),
        ] {
            assert_eq!(
                receipt_matches(
                    repo,
                    "keys-i",
                    7,
                    9,
                    Some(&issue_value),
                    Some(&comment_value),
                    Some(&permission_value),
                ),
                valid
            );
        }
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
    fn central_configuration_keeps_credentials_out_of_target_repositories() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        fs::create_dir_all(temporary.path().join(".github"))?;
        fs::write(
            temporary.path().join(".github/dependabot.yaml"),
            "version: 2\n",
        )?;
        let source = SourceRef::parse(&format!("keys-i/rady@{}", "a".repeat(40)))?;
        let files = local_files(
            temporary.path(),
            &source,
            &["check".to_owned()],
            Identity::Rady,
            true,
        )?;
        assert_eq!(files.len(), 1);
        let configuration = files
            .iter()
            .find(|(path, _)| path.ends_with(".github/rady.json"))
            .map(|(_, content)| serde_json::from_str::<Value>(content))
            .expect("generated Rady configuration")?;
        assert_eq!(configuration["schema"], 1);
        assert_eq!(configuration["source"], source.joined());
        assert_eq!(configuration["checks"], json!(["check"]));
        assert!(configuration["agreement"].is_null());
        assert!(
            !files
                .keys()
                .any(|path| path.ends_with(".github/dependabot.yml"))
        );

        let orchestrator = include_str!("../.github/workflows/orchestrate.yml");
        let solver = include_str!("../.github/workflows/solve.yml");
        let selector = include_str!("../.github/workflows/scripts/select-central-targets.sh");
        for secret in [
            "RADY_APP_PRIVATE_KEY",
            "RADY_APP_CLIENT_ID",
            "RADY_APP_SLUG",
            "RADY_GEMINI_API_KEY",
            "RADY_CEREBRAS_API_KEY",
            "RADY_XAI_API_KEY",
        ] {
            assert!(orchestrator.contains(secret));
            assert!(!serde_json::to_string(&configuration)?.contains(secret));
        }
        assert!(orchestrator.contains("target/release/rady agent sweep --owner keys-i"));
        assert!(orchestrator.contains("uses: ./.github/workflows/solve.yml"));
        assert!(orchestrator.contains("permission-issues: read"));
        assert!(!orchestrator.contains("runs-on: self-hosted"));
        assert!(!solver.contains("runs-on: self-hosted"));
        assert!(!orchestrator.contains("actions/cache@"));
        assert!(!solver.contains("actions/cache@"));
        assert!(solver.contains("repo:"));
        assert!(solver.contains("repo-owner:"));
        assert!(solver.contains("repo-name:"));
        assert!(solver.contains("owner: ${{ inputs.repo-owner }}"));
        assert!(solver.contains("repositories: ${{ inputs.repo-name }}"));
        assert!(selector.contains(".agreement.terms == \"2026-09-23\""));
        assert!(selector.contains(".agreement.privacy == \"2026-09-23\""));
        assert!(selector.contains("collaborators/$signer/permission"));
        assert!(selector.contains("Rady service agreement acceptance"));
        assert!(selector.contains(".head.repo.full_name == $repository"));
        assert!(selector.contains(".author_association == \"OWNER\""));
        Ok(())
    }
}
