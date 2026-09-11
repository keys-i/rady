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
        if missing && output.stderr.contains("(HTTP 404)") {
            return Ok(None);
        }
        bail!("GitHub request failed; check CLI login and repository access");
    }
    Ok(Some(output.stdout))
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
    );
    let output = output?;
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
}
