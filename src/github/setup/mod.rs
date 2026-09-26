use std::path::Path;

use anyhow::{Context, anyhow, bail};
use serde_json::{Value, json};

use crate::Result;
use crate::github;
use crate::github::apps;

mod consent;
mod files;

pub(crate) use consent::verified_configuration;
use consent::{PRIVACY_VERSION, TERMS_VERSION};
pub use files::{checks, local_files};
use files::{existing_configuration, refuse_existing_configuration, setup_files, write_setup_file};

const TRUSTED_SOLVER_REPOSITORY: &str = "keys-i/koelu";
pub const TERMS_URL: &str = "https://github.com/keys-i/koelu/blob/main/docs/TERMS.md";
pub const PRIVACY_URL: &str = "https://github.com/keys-i/koelu/blob/main/docs/PRIVACY.md";

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
            .ok_or_else(|| anyhow!("--solver-ref requires keys-i/koelu@40_LOWERCASE_COMMIT_SHA"))?;
        github::validate_repository(repository)?;
        if repository != TRUSTED_SOLVER_REPOSITORY || !valid_commit(commit) {
            bail!("--solver-ref requires keys-i/koelu@40_LOWERCASE_COMMIT_SHA");
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
    overwrite: bool,
    accept_terms: bool,
) -> Result<()> {
    ensure_directory_repository(directory, repo)?;
    let existing = existing_configuration(directory)?;
    let existing_agreement = existing
        .as_ref()
        .filter(|configuration| consent::accepted_configuration(configuration))
        .and_then(|configuration| configuration.get("agreement"));
    setup_files(directory, source, required, overwrite, existing_agreement)?;
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
    let reuse_agreement = match existing.as_ref() {
        Some(configuration) => consent::verified_existing_configuration(repo, configuration)?,
        None => false,
    };
    if !accept_terms && !reuse_agreement {
        bail!("read {TERMS_URL} and {PRIVACY_URL}, then rerun with --accept-terms if you agree");
    }
    if !overwrite && existing.is_some() && !reuse_agreement {
        bail!("refusing to overwrite existing .github/koelu.json");
    }
    let agreement = if reuse_agreement {
        existing
            .as_ref()
            .and_then(|configuration| configuration.get("agreement"))
            .cloned()
            .ok_or_else(|| anyhow!("existing Koelu agreement was missing"))?
    } else {
        let app = apps::public_app(apps::KOELU_SLUG)?;
        apps::require_app_owner(&app)?;
        apps::require_permissions(&app)?;
        apps::open_installation(apps::KOELU_SLUG, repo)?;
        consent::agreement(repo)?
    };
    let files = setup_files(directory, source, required, overwrite, Some(&agreement))?;
    for (path, content) in files {
        let replace = overwrite && path.ends_with(".github/koelu.json");
        write_setup_file(&path, content.as_bytes(), replace)?;
    }
    Ok(())
}

/// Resolve an explicit repository or the repository containing the current checkout
pub fn resolve_repository_in(value: Option<&str>, directory: &Path) -> Result<String> {
    let local = repository_in_directory(directory)?;
    match value {
        Some(repository) => {
            github::validate_repository(repository)?;
            if !same_repository(repository, &local) {
                bail!(
                    "--directory belongs to {local}, not {repository}; choose the matching checkout"
                );
            }
            Ok(local)
        }
        None => Ok(local),
    }
}

pub fn ensure_directory_repository(directory: &Path, repo: &str) -> Result<()> {
    github::validate_repository(repo)?;
    let local = repository_in_directory(directory)?;
    if !same_repository(repo, &local) {
        bail!("--directory belongs to {local}, not {repo}; choose the matching checkout");
    }
    Ok(())
}

pub fn has_verified_agreement(repo: &str, directory: &Path) -> Result<bool> {
    github::validate_repository(repo)?;
    match existing_configuration(directory)? {
        Some(configuration) => consent::verified_existing_configuration(repo, &configuration),
        None => Ok(false),
    }
}

fn same_repository(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn repository_in_directory(directory: &Path) -> Result<String> {
    let root = directory
        .canonicalize()
        .context("--directory must be an existing directory")?;
    if !root.is_dir() {
        bail!("--directory must be a directory");
    }
    let arguments = vec![
        "repo".to_owned(),
        "view".to_owned(),
        "--json".to_owned(),
        "nameWithOwner".to_owned(),
    ];
    let response = github::gh_in_directory(&arguments, None, false, &root)?
        .ok_or_else(|| anyhow!("GitHub returned no repository"))?;
    repository_from_view(&response)
}

/// Resolve explicit CI checks or evidence from the repository's default branch
pub fn resolve_checks(repo: &str, explicit: &[String]) -> Result<Vec<String>> {
    github::validate_repository(repo)?;
    if !explicit.is_empty() {
        return checks(explicit);
    }
    let repository = github::api(&format!("repos/{repo}"), None, "GET", false)?
        .ok_or_else(|| anyhow!("repository response was empty"))?;
    let branch = repository_default_branch(repo, &repository)?;
    let branch_response = github::api(
        &format!("repos/{repo}/branches/{}", percent_encode(branch)),
        None,
        "GET",
        false,
    )?
    .ok_or_else(|| anyhow!("default branch response was empty"))?;
    let commit = branch_response["commit"]["sha"]
        .as_str()
        .filter(|sha| valid_commit(sha))
        .ok_or_else(|| anyhow!("default branch has no valid commit"))?;
    if let Some(required) = github::api(
        &format!(
            "repos/{repo}/branches/{}/protection/required_status_checks",
            percent_encode(branch)
        ),
        None,
        "GET",
        true,
    )? {
        let required = required_check_names(&required)?;
        if !required.is_empty() {
            return Ok(required);
        }
    }
    let runs = github::api(
        &format!("repos/{repo}/commits/{commit}/check-runs?per_page=100"),
        None,
        "GET",
        false,
    )?
    .ok_or_else(|| anyhow!("latest check-runs response was empty"))?;
    let statuses = github::api(
        &format!("repos/{repo}/commits/{commit}/statuses?per_page=100"),
        None,
        "GET",
        false,
    )?
    .ok_or_else(|| anyhow!("latest statuses response was empty"))?;
    latest_check_names(&runs, &statuses)
}

fn repository_from_view(response: &str) -> Result<String> {
    let repository = serde_json::from_str::<Value>(response)
        .context("GitHub returned an invalid repository response")?["nameWithOwner"]
        .as_str()
        .ok_or_else(|| anyhow!("GitHub returned no repository name"))?
        .to_owned();
    github::validate_repository(&repository)?;
    Ok(repository)
}

fn repository_default_branch<'a>(repo: &str, response: &'a Value) -> Result<&'a str> {
    if response["full_name"]
        .as_str()
        .is_none_or(|name| !name.eq_ignore_ascii_case(repo))
    {
        bail!("GitHub returned a different repository");
    }
    response["default_branch"]
        .as_str()
        .filter(|branch| {
            !branch.is_empty() && branch.len() <= 255 && !branch.chars().any(char::is_control)
        })
        .ok_or_else(|| anyhow!("repository has no valid default branch"))
}

fn required_check_names(response: &Value) -> Result<Vec<String>> {
    let contexts = response["contexts"]
        .as_array()
        .ok_or_else(|| anyhow!("required status checks omitted contexts"))?;
    let check_rows = response["checks"].as_array().map_or(&[][..], Vec::as_slice);
    let names = contexts
        .iter()
        .filter_map(Value::as_str)
        .chain(
            check_rows
                .iter()
                .filter_map(|check| check["context"].as_str()),
        )
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.len() != contexts.len() + check_rows.len() {
        bail!("required status checks contain invalid names");
    }
    checks(&names)
}

fn latest_check_names(runs: &Value, statuses: &Value) -> Result<Vec<String>> {
    let runs = runs["check_runs"]
        .as_array()
        .ok_or_else(|| anyhow!("latest check-runs response omitted check_runs"))?;
    let statuses = statuses
        .as_array()
        .ok_or_else(|| anyhow!("latest statuses response was not a list"))?;
    if runs.len() >= 100 || statuses.len() >= 100 {
        bail!("latest CI evidence exceeds the setup limit; pass --check explicitly");
    }
    let names = runs
        .iter()
        .chain(statuses)
        .map(|row| {
            row["name"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| row["context"].as_str().map(str::to_owned))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| anyhow!("latest CI evidence contains an invalid name"))?;
    if names.is_empty() {
        bail!("no CI evidence found; pass --check explicitly");
    }
    checks(&names)
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    repo: &str,
    source: &SourceRef,
    required: &[String],
    directory: &Path,
    overwrite: bool,
    apply: bool,
    accept_terms: bool,
) -> Result<Value> {
    github::validate_repository(repo)?;
    let required = checks(required)?;
    refuse_existing_configuration(directory, overwrite)?;
    let agreement = existing_configuration(directory)?
        .filter(consent::accepted_configuration)
        .and_then(|configuration| configuration.get("agreement").cloned());
    let files = setup_files(directory, source, &required, overwrite, agreement.as_ref())?;
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
        "app_permissions": apps::permissions(),
        "app": apps::KOELU_SLUG,
        "overwrite": overwrite,
        "apply": apply,
    });
    if apply {
        install(repo, source, &required, directory, overwrite, accept_terms)?;
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
            (format!("keys-i/koelu@{}", "a".repeat(40)), true),
            (format!("owner/repo@{}", "a".repeat(40)), false),
            (format!("keys-i/koelu@{}", "A".repeat(40)), false),
            ("keys-i/koelu@short".to_owned(), false),
            (format!("keys-i/koelu@{}", "g".repeat(40)), false),
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
        assert_eq!(source.joined(), format!("keys-i/koelu@{commit}"));
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
                json!({"full_name": "keys-i/koelu", "default_branch": "main"}),
                true,
            ),
            (
                json!({"full_name": "keys-i/koelu", "default_branch": "feature/a"}),
                true,
            ),
            (
                json!({"full_name": "other/koelu", "default_branch": "main"}),
                false,
            ),
            (
                json!({"full_name": "keys-i/koelu", "default_branch": ""}),
                false,
            ),
            (
                json!({"full_name": "keys-i/koelu", "default_branch": "bad\nbranch"}),
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
        let source = SourceRef::parse(&format!("keys-i/koelu@{}", "a".repeat(40)))?;
        let files = local_files(temporary.path(), &source, &["check".to_owned()], true)?;
        assert_eq!(files.len(), 1);
        let configuration = files
            .iter()
            .find(|(path, _)| path.ends_with(".github/koelu.json"))
            .map(|(_, content)| serde_json::from_str::<Value>(content))
            .expect("generated Koelu configuration")?;
        assert_eq!(configuration["schema"], 1);
        assert_eq!(configuration["source"], source.joined());
        assert_eq!(configuration["checks"], json!(["check"]));
        assert!(configuration["agreement"].is_null());
        assert!(
            !files
                .keys()
                .any(|path| path.ends_with(".github/dependabot.yml"))
        );

        let orchestrator = include_str!("../../../.github/workflows/orchestrate.yml");
        for secret in [
            "KOELU_APP_PRIVATE_KEY",
            "KOELU_APP_CLIENT_ID",
            "KOELU_APP_SLUG",
            "KOELU_GEMINI_API_KEY",
            "KOELU_CEREBRAS_API_KEY",
            "KOELU_XAI_API_KEY",
            "KOELU_GROQ_API_KEY",
            "KOELU_CLOUDFLARE_API_TOKEN",
            "KOELU_CLOUDFLARE_ACCOUNT_ID",
            "KOELU_OPENROUTER_API_KEY",
            "KOELU_GEMINI_PRIVATE_OK",
            "KOELU_CEREBRAS_PRIVATE_OK",
            "KOELU_XAI_PRIVATE_OK",
            "KOELU_GROQ_PRIVATE_OK",
            "KOELU_CLOUDFLARE_PRIVATE_OK",
            "KOELU_OPENROUTER_PRIVATE_OK",
        ] {
            assert!(orchestrator.contains(secret));
            assert!(!serde_json::to_string(&configuration)?.contains(secret));
        }
        for local in ["KOELU_LAYA_ENABLED", "KOELU_LAYA_API_KEY"] {
            assert!(!serde_json::to_string(&configuration)?.contains(local));
            assert!(!orchestrator.contains(local));
        }
        assert!(orchestrator.contains("koelu agent serve --once"));
        assert!(orchestrator.contains("koelu agent targets --max-reviews 4"));
        assert!(orchestrator.contains("uses: ./.github/workflows/solve.yml"));
        assert!(orchestrator.contains("max-parallel: 4"));
        assert!(!orchestrator.contains("--owner"));
        assert!(!orchestrator.contains("target/release/koelu agent sweep"));
        assert!(!orchestrator.contains("target/release/koelu agent respond"));
        assert!(!orchestrator.contains("runs-on: self-hosted"));
        assert!(!orchestrator.contains("actions/cache@"));
        assert!(orchestrator.contains("permissions:\n  contents: read"));
        assert!(!orchestrator.contains("contents: write"));
        Ok(())
    }

    #[test]
    fn setup_response_parsing_is_bounded_and_unambiguous() {
        for (response, expected) in [
            (r#"{"nameWithOwner":"keys-i/koelu"}"#, Some("keys-i/koelu")),
            (r#"{"nameWithOwner":"bad"}"#, None),
            (r#"{"nameWithOwner":null}"#, None),
            ("not json", None),
        ] {
            assert_eq!(
                repository_from_view(response).ok().as_deref(),
                expected,
                "{response}"
            );
        }

        for (required, latest, valid) in [
            (
                json!({"contexts": ["test"], "checks": [{"context": "lint"}]}),
                json!({"check_runs": []}),
                true,
            ),
            (json!({"contexts": []}), json!({"check_runs": []}), false),
            (
                json!({"contexts": ["\n"]}),
                json!({"check_runs": []}),
                false,
            ),
            (
                json!({"contexts": (0..33).map(|index| format!("check-{index}")).collect::<Vec<_>>() }),
                json!({"check_runs": []}),
                false,
            ),
        ] {
            assert_eq!(required_check_names(&required).is_ok(), valid, "{required}");
            let statuses = json!([]);
            assert!(latest_check_names(&latest, &statuses).is_err());
        }

        let latest = json!({"check_runs": [{"name": "test"}, {"name": "lint"}]});
        assert_eq!(
            latest_check_names(&latest, &json!([{"context": "test"}])).expect("checks"),
            ["test", "lint"]
        );
    }

    #[test]
    fn repository_matching_is_case_insensitive_but_exact() {
        for (left, right, expected) in [
            ("keys-i/koelu", "keys-i/koelu", true),
            ("keys-i/koelu", "keys-i/other", false),
            ("keys-i/koelu", "other/koelu", false),
        ] {
            assert_eq!(same_repository(left, right), expected, "{left} / {right}");
        }
    }
}
