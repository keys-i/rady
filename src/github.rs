use std::collections::BTreeMap;
use std::env;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use regex::Regex;
use serde_json::Value;

use crate::Result;
use crate::agent;

#[derive(Clone, Debug)]
pub struct GitHub {
    repo: String,
    token: String,
}

impl GitHub {
    pub fn new(repo: &str, token: &str) -> Result<Self> {
        validate_repository(repo)?;
        if token.is_empty() {
            bail!("a GitHub App installation token is required");
        }
        Ok(Self {
            repo: repo.to_owned(),
            token: token.to_owned(),
        })
    }

    #[must_use]
    pub fn repo(&self) -> &str {
        &self.repo
    }

    pub fn api(&self, path: &str, payload: Option<&Value>, method: &str) -> Result<Value> {
        let endpoint = format!("repos/{}/{}", self.repo, path);
        api_with_token(&endpoint, payload, method, false, Some(&self.token), None)?
            .ok_or_else(|| anyhow!("GitHub returned no response"))
    }

    pub fn pages(&self, path: &str, key: Option<&str>) -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        for page in 1..32 {
            let separator = if path.contains('?') { '&' } else { '?' };
            let value = self.api(
                &format!("{path}{separator}per_page=100&page={page}"),
                None,
                "GET",
            )?;
            let batch = match key {
                Some(key) => value
                    .get(key)
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("GitHub response omitted {key}"))?,
                None => value
                    .as_array()
                    .ok_or_else(|| anyhow!("GitHub response was not a list"))?,
            };
            rows.extend(batch.iter().cloned());
            if batch.len() < 100 {
                return Ok(rows);
            }
        }
        bail!("GitHub results exceeded the review limit; review manually")
    }
}

pub fn validate_repository(value: &str) -> Result<()> {
    let expression = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9-]{0,38}/[A-Za-z0-9_.-]{1,100}$")?;
    if !expression.is_match(value) || matches!(value.split('/').nth(1), Some("." | "..")) {
        bail!("use an explicit OWNER/REPO");
    }
    Ok(())
}

pub fn gh(arguments: &[String], data: Option<&str>, missing: bool) -> Result<Option<String>> {
    gh_with_token(arguments, data, missing, None, None)
}

fn gh_with_token(
    arguments: &[String],
    data: Option<&str>,
    missing: bool,
    token: Option<&str>,
    cancel_file: Option<&Path>,
) -> Result<Option<String>> {
    let binary = agent::which("gh").ok_or_else(|| anyhow!("install GitHub CLI"))?;
    let mut ambient_auth = BTreeMap::new();
    for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = env::var(name) {
            ambient_auth.insert(name.to_owned(), value);
        }
    }
    let environment = github_environment(token, &ambient_auth);
    let output = agent::execute(
        binary.as_os_str(),
        arguments,
        Path::new("."),
        data.unwrap_or_default().as_bytes(),
        Duration::from_secs(180),
        &environment,
        false,
        cancel_file,
    )?;
    if output.code != 0 {
        if is_missing_response(&output.stderr, missing) {
            return Ok(None);
        }
        bail!("{}", github_failure(&output.stderr, output.code));
    }
    Ok(Some(output.stdout))
}

fn is_missing_response(stderr: &str, missing: bool) -> bool {
    missing && github_status(stderr) == Some(404)
}

fn github_status(stderr: &str) -> Option<u16> {
    Regex::new(r"(?i)(?:http(?:/[0-9.]+)?|status(?: code)?)\D{0,12}([1-5][0-9]{2})")
        .ok()?
        .captures(stderr)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

fn github_failure(stderr: &str, code: i32) -> String {
    match github_status(stderr) {
        Some(401) => "GitHub authentication failed (401); sign in with gh auth login or refresh the App token".to_owned(),
        Some(403) => {
            "GitHub access denied (403); confirm the App installation or CLI account can access the repository".to_owned()
        }
        Some(404) => {
            "GitHub repository or API resource was not found (404); confirm the repository name and App installation".to_owned()
        }
        Some(422) => {
            "GitHub rejected the request (422); check the repository configuration and requested change".to_owned()
        }
        Some(429) => "GitHub rate limit reached (429); wait before retrying".to_owned(),
        _ if stderr.to_ascii_lowercase().contains("gh auth login")
            || stderr
                .to_ascii_lowercase()
                .contains("not logged into any github hosts") =>
        {
            "GitHub CLI is not authenticated; run gh auth login".to_owned()
        }
        _ => format!("GitHub request failed (exit {code}); check gh auth status and repository access"),
    }
}

fn safe_endpoint_label(endpoint: &str) -> String {
    const LIMIT: usize = 160;
    let mut redact_next = false;
    let mut label = String::new();
    for (index, segment) in endpoint.split('/').enumerate() {
        if index != 0 {
            label.push('/');
        }
        if redact_next {
            label.push_str("<redacted>");
            redact_next = false;
            continue;
        }
        let segment: String = segment
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        redact_next = segment == "app-manifests";
        label.push_str(&segment);
    }
    if label.chars().count() <= LIMIT {
        return label;
    }
    let shortened: String = label.chars().take(LIMIT - 1).collect();
    format!("{shortened}…")
}

fn github_environment(
    token: Option<&str>,
    ambient_auth: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    match token {
        Some(token) => BTreeMap::from([("GH_TOKEN".to_owned(), token.to_owned())]),
        None => ambient_auth
            .iter()
            .filter(|(name, _)| matches!(name.as_str(), "GH_TOKEN" | "GITHUB_TOKEN"))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    }
}

pub fn api(
    endpoint: &str,
    payload: Option<&Value>,
    method: &str,
    missing: bool,
) -> Result<Option<Value>> {
    api_with_token(endpoint, payload, method, missing, None, None)
}

pub fn api_cancellable(
    endpoint: &str,
    payload: Option<&Value>,
    method: &str,
    missing: bool,
    cancel_file: &Path,
) -> Result<Option<Value>> {
    api_with_token(endpoint, payload, method, missing, None, Some(cancel_file))
}

fn api_with_token(
    endpoint: &str,
    payload: Option<&Value>,
    method: &str,
    missing: bool,
    token: Option<&str>,
    cancel_file: Option<&Path>,
) -> Result<Option<Value>> {
    let mut arguments = vec![
        "api".to_owned(),
        "--method".to_owned(),
        method.to_owned(),
        endpoint.to_owned(),
    ];
    if payload.is_some() {
        arguments.extend(["--input".to_owned(), "-".to_owned()]);
    }
    let output = gh_with_token(
        &arguments,
        payload.map(serde_json::to_string).transpose()?.as_deref(),
        missing,
        token,
        cancel_file,
    )
    .with_context(|| format!("GitHub API {method} {}", safe_endpoint_label(endpoint)))?;
    let Some(output) = output.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    serde_json::from_str(&output)
        .map(Some)
        .context("GitHub returned invalid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_validation_uses_a_compact_case_table() {
        for (value, valid) in [
            ("owner/repo", true),
            ("owner/repo.name", true),
            ("owner", false),
            ("/repo", false),
            ("owner/..", false),
            ("owner/repo/extra", false),
        ] {
            assert_eq!(validate_repository(value).is_ok(), valid, "{value}");
        }
    }

    #[test]
    fn github_environment_keeps_only_authorisation() {
        let ambient = BTreeMap::from([
            ("GH_TOKEN".to_owned(), "ambient-gh".to_owned()),
            ("GITHUB_TOKEN".to_owned(), "ambient-github".to_owned()),
            ("GH_HOST".to_owned(), "attacker.example".to_owned()),
            ("GH_REPO".to_owned(), "attacker/repo".to_owned()),
        ]);
        for (token, expected) in [
            (
                None,
                BTreeMap::from([
                    ("GH_TOKEN".to_owned(), "ambient-gh".to_owned()),
                    ("GITHUB_TOKEN".to_owned(), "ambient-github".to_owned()),
                ]),
            ),
            (
                Some("app-token"),
                BTreeMap::from([("GH_TOKEN".to_owned(), "app-token".to_owned())]),
            ),
        ] {
            assert_eq!(github_environment(token, &ambient), expected);
        }
    }

    #[test]
    fn github_failures_are_classified_without_echoing_stderr() {
        let token = "arbitrary-token-that-must-not-escape";
        for (stderr, status, missing, message, endpoint, hidden) in [
            (
                "request failed (HTTP 404)",
                Some(404),
                true,
                "not found (404)",
                "repos/owner/repo",
                None,
            ),
            (
                "HTTP/2 401 unauthorized",
                Some(401),
                false,
                "authentication failed (401)",
                "repos/owner/repo",
                None,
            ),
            (
                "status code: 403",
                Some(403),
                false,
                "access denied (403)",
                "repos/owner/repo",
                None,
            ),
            (
                "HTTP 422",
                Some(422),
                false,
                "rejected the request (422)",
                "repos/owner/repo",
                None,
            ),
            (
                "HTTP 429",
                Some(429),
                false,
                "rate limit reached (429)",
                "repos/owner/repo",
                None,
            ),
            (
                "To get started with GitHub CLI, please run: gh auth login",
                None,
                false,
                "CLI is not authenticated",
                "repos/owner/repo",
                None,
            ),
            (
                "connection closed with secret arbitrary-token-that-must-not-escape",
                None,
                false,
                "request failed (exit 7)",
                "app-manifests/one-time-code\nwith-control/conversions/abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz",
                Some("one-time-code"),
            ),
        ] {
            assert_eq!(github_status(stderr), status, "{stderr}");
            assert_eq!(is_missing_response(stderr, true), missing, "{stderr}");
            let failure = github_failure(stderr, 7);
            assert!(failure.contains(message), "{stderr}: {failure}");
            assert!(!failure.contains(token), "{stderr}: {failure}");
            let label = safe_endpoint_label(endpoint);
            assert!(label.chars().count() <= 160, "{label}");
            assert!(!label.chars().any(char::is_control), "{label}");
            if let Some(hidden) = hidden {
                assert!(!label.contains(hidden), "{label}");
            }
        }
    }
}
