use crate::Result;
use crate::github;
use anyhow::{anyhow, bail};
use regex::Regex;
use serde_json::{Value, json};

pub const APP_OWNER: &str = "keys-i";
pub const KOELU_SLUG: &str = "koelu";

pub fn permissions() -> Value {
    json!({
        "administration": "read",
        "contents": "write",
        "checks": "read",
        "issues": "write",
        "metadata": "read",
        "statuses": "read",
        "pull_requests": "write"
    })
}

pub fn require_app_owner(app: &Value) -> Result<()> {
    if app["owner"]["login"]
        .as_str()
        .is_none_or(|login| !login.eq_ignore_ascii_case(APP_OWNER))
    {
        bail!("the GitHub App must be registered under {APP_OWNER}; no credentials were changed");
    }
    Ok(())
}

pub fn require_permissions(app: &Value) -> Result<()> {
    let actual = app["permissions"]
        .as_object()
        .ok_or_else(|| anyhow!("App response has no permissions"))?;
    let configured = permissions();
    let expected = configured
        .as_object()
        .ok_or_else(|| anyhow!("internal permissions are invalid"))?;
    if actual.len() != expected.len()
        || expected
            .iter()
            .any(|(name, level)| actual.get(name) != Some(level))
    {
        bail!(
            "the App permissions must exactly be Administration, Checks, Metadata and Commit statuses read, plus Contents, Issues and Pull requests write"
        );
    }
    Ok(())
}

pub fn validate_slug(slug: &str) -> Result<()> {
    if !Regex::new(r"^[a-z0-9-]+$")?.is_match(slug) {
        bail!("the configured GitHub App slug is invalid");
    }
    Ok(())
}

pub fn require_public_app(app: &Value) -> Result<()> {
    if app["public"].as_bool() != Some(true) {
        bail!("the GitHub App must be public");
    }
    Ok(())
}

pub fn require_app_identity(app: &Value, slug: &str) -> Result<()> {
    validate_slug(slug)?;
    require_app_owner(app)?;
    require_public_app(app)?;
    if app["slug"]
        .as_str()
        .is_none_or(|actual| !actual.eq_ignore_ascii_case(slug))
    {
        bail!("the App credentials do not belong to the configured Koelu App");
    }
    require_permissions(app)
}

pub fn public_app(slug: &str) -> Result<Value> {
    validate_slug(slug)?;
    github::public_app(slug)
}

pub fn open_installation(slug: &str, repo: &str) -> Result<()> {
    github::validate_repository(repo)?;
    validate_slug(slug)?;
    let install = format!("https://github.com/apps/{slug}/installations/new");
    eprintln!("Install Koelu with Only select repositories -> {repo}");
    open_browser(&install);
    Ok(())
}

fn open_browser(url: &str) {
    let candidates: &[(&'static str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(target_os = "windows") {
        &[("cmd", &["/C", "start", ""])]
    } else {
        &[("xdg-open", &[])]
    };
    for (program, arguments) in candidates {
        if let Some(binary) = crate::agent::which(program) {
            let _ = browser_command(binary, arguments, url).spawn();
            return;
        }
    }
}

fn browser_command(
    binary: impl AsRef<std::ffi::OsStr>,
    arguments: &[&str],
    url: &str,
) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command
        .env_clear()
        .envs(crate::agent::safe_environment())
        .args(arguments)
        .arg(url);
    command
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn browser_command_rebuilds_a_safe_environment() {
        let command = browser_command("open", &[], "https://example.test");
        let environment = command.get_envs().collect::<BTreeMap<_, _>>();
        assert!(environment.contains_key(std::ffi::OsStr::new("PATH")));
        assert!(!environment.contains_key(std::ffi::OsStr::new("KOELU_APP_PRIVATE_KEY")));
    }

    #[test]
    fn app_permissions_reserve_write_access_for_approved_delivery() -> Result<()> {
        let mut app = json!({"permissions": permissions(), "owner": {"login": APP_OWNER}, "public": true, "slug": KOELU_SLUG});
        assert_eq!(app["permissions"]["contents"], "write");
        require_app_owner(&app)?;
        require_permissions(&app)?;
        app["permissions"]["contents"] = json!("read");
        assert!(require_permissions(&app).is_err());
        app["permissions"]["contents"] = json!("none");
        assert!(require_permissions(&app).is_err());
        app["permissions"]["contents"] = json!("write");
        app["permissions"]["issues"] = json!("read");
        assert!(require_permissions(&app).is_err());
        app["permissions"]["issues"] = json!("write");
        app["permissions"]["pull_requests"] = json!("read");
        assert!(require_permissions(&app).is_err());
        app["permissions"]["pull_requests"] = json!("write");
        app["permissions"]["metadata"] = json!("write");
        assert!(require_permissions(&app).is_err());
        app["permissions"]["metadata"] = json!("read");
        app["permissions"]["workflows"] = json!("read");
        assert!(require_permissions(&app).is_err());
        app["permissions"]
            .as_object_mut()
            .unwrap()
            .remove("workflows");
        require_app_identity(&app, KOELU_SLUG)?;
        app["public"] = json!(false);
        assert!(require_app_identity(&app, KOELU_SLUG).is_err());
        assert!(validate_slug("Koelu").is_err());
        Ok(())
    }
}
