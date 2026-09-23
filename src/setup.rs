use std::path::Path;

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

use crate::Result;
use crate::apps::{self, Identity};
use crate::github;

mod consent;
mod files;

pub(crate) use consent::verified_configuration;
use consent::{PRIVACY_VERSION, TERMS_VERSION};
pub use files::{checks, local_files};
use files::{setup_files, write_setup_file};

const TRUSTED_SOLVER_REPOSITORY: &str = "keys-i/rady";
const TERMS_URL: &str = "https://github.com/keys-i/rady/blob/main/docs/TERMS.md";
const PRIVACY_URL: &str = "https://github.com/keys-i/rady/blob/main/docs/PRIVACY.md";

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
    verify_existing_app(existing)?;
    // GitHub reserves installation lookup for App JWTs; central token creation verifies access
    let agreement = consent::agreement(repo)?;
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
    use std::fs;

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
        assert!(orchestrator.contains(
            "    permissions:\n      contents: read\n      pull-requests: write\n    strategy:"
        ));
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
