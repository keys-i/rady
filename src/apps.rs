use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use crate::Result;
use crate::github;
use crate::ui::RADY_DUCK_PNG;
use anyhow::{Context, anyhow, bail};
use clap::ValueEnum;
use regex::Regex;
use serde_json::{Value, json};

pub const APP_OWNER: &str = "keys-i";
pub const RADYYBOT_SLUG: &str = "radyybot";
const DUCK_ROUTE: &str = "/rady-duck.png";

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Identity {
    Dependasolver,
    Rady,
}

impl Identity {
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Dependasolver => "DEPENDASOLVER",
            Self::Rady => "RADY",
        }
    }

    #[must_use]
    pub const fn display(self) -> &'static str {
        match self {
            Self::Dependasolver => "Dependasolver",
            Self::Rady => "Rady",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Dependasolver => "dependasolver",
            Self::Rady => RADYYBOT_SLUG,
        }
    }
}

pub fn permissions() -> Value {
    json!({
        "administration": "read",
        "contents": "read",
        "checks": "read",
        "issues": "write",
        "statuses": "read",
        "pull_requests": "write"
    })
}

pub fn manifest(repo: &str, callback: &str, identity: Identity) -> Value {
    json!({
        "name": match identity {
            Identity::Dependasolver => identity.display(),
            Identity::Rady => "radyybot",
        },
        "url": format!("https://github.com/{repo}"),
        "description": "Rady reviews dependency updates from their diff and CI evidence, enables protected auto-merge when safe, and answers trusted @radyybot requests.",
        "public": true,
        "hook_attributes": {"active": false, "url": format!("https://github.com/{repo}")},
        "redirect_url": callback,
        "default_permissions": permissions(),
        "default_events": []
    })
}

pub fn callback_code(path: &str, route: &str, state: &str) -> Result<String> {
    let (actual_path, query) = path.split_once('?').unwrap_or((path, ""));
    if actual_path != route {
        bail!("invalid App registration callback");
    }
    let values: BTreeMap<_, _> = query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| Ok((percent_decode(key)?, percent_decode(value)?)))
        .collect::<Result<_>>()?;
    let returned_state = values
        .get("state")
        .ok_or_else(|| anyhow!("invalid App registration callback"))?;
    let code = values
        .get("code")
        .ok_or_else(|| anyhow!("invalid App registration callback"))?;
    if !constant_time_equal(returned_state.as_bytes(), state.as_bytes())
        || !(20..=256).contains(&code.len())
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        bail!("invalid App registration callback");
    }
    Ok(code.clone())
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
    let valid = permissions()
        .as_object()
        .ok_or_else(|| anyhow!("internal permissions are invalid"))?
        .iter()
        .all(|(name, level)| {
            let expected = level.as_str().unwrap_or_default();
            actual
                .get(name)
                .and_then(Value::as_str)
                .is_some_and(|value| value == "write" || (expected == "read" && value == "read"))
        });
    if !valid {
        bail!(
            "the App needs Administration, Checks, Contents and Commit statuses read, plus Issues and Pull requests write"
        );
    }
    Ok(())
}

pub fn public_app(slug: &str) -> Result<Value> {
    if !Regex::new(r"^[a-z0-9-]+$")?.is_match(slug) {
        bail!("the configured GitHub App slug is invalid");
    }
    github::api(&format!("apps/{slug}"), None, "GET", false)?
        .ok_or_else(|| anyhow!("could not verify the existing public App"))
}

pub fn open_installation(slug: &str, repo: &str) -> Result<()> {
    github::validate_repository(repo)?;
    if !Regex::new(r"^[a-z0-9-]+$")?.is_match(slug) {
        bail!("the configured GitHub App slug is invalid");
    }
    let install = format!("https://github.com/apps/{slug}/installations/new");
    eprintln!("Install radyybot with Only select repositories -> {repo}");
    open_browser(&install);
    Ok(())
}

pub fn register_app(repo: &str, identity: Identity) -> Result<Value> {
    let owner = github::api(&format!("users/{APP_OWNER}"), None, "GET", false)?
        .ok_or_else(|| anyhow!("could not resolve App owner"))?;
    let owner_type = owner["type"].as_str().unwrap_or_default();
    if !matches!(owner_type, "User" | "Organization") {
        bail!("cannot register a GitHub App under {APP_OWNER}");
    }
    if owner_type == "User" {
        let user = github::api("user", None, "GET", false)?
            .ok_or_else(|| anyhow!("could not resolve current GitHub user"))?;
        if user["login"]
            .as_str()
            .is_none_or(|login| !login.eq_ignore_ascii_case(APP_OWNER))
        {
            bail!("sign in to GitHub CLI and the browser as {APP_OWNER}");
        }
    }

    let state = random_token(32)?;
    let nonce = random_token(32)?;
    let route = format!("/callback/{}", random_token(24)?);
    let start = format!("/start/{}", random_token(24)?);
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let host = format!("127.0.0.1:{}", listener.local_addr()?.port());
    let settings = if owner_type == "User" {
        "settings/apps/new".to_owned()
    } else {
        format!("organizations/{APP_OWNER}/settings/apps/new")
    };
    let action = format!(
        "https://github.com/{settings}?state={}",
        percent_encode(&state)
    );
    let config = manifest(repo, &format!("http://{host}{route}"), identity);
    let form = setup_page(
        "Connect your repository",
        &format!(
            r#"<p>Create a public GitHub App owned by <strong>{APP_OWNER}</strong> for <strong>{}</strong>. It reviews dependency pull requests, turns on GitHub auto-merge only after the checks pass, and answers trusted collaborators who write <code>@radyybot</code>.</p><dl><div><dt>Administration</dt><dd>Read branch protection and repository settings</dd></div><div><dt>Checks</dt><dd>Read CI results</dd></div><div><dt>Contents</dt><dd>Read changed files and Rady configuration</dd></div><div><dt>Commit statuses</dt><dd>Read legacy status checks</dd></div><div><dt>Issues</dt><dd>Read prompts and post replies</dd></div><div><dt>Pull requests</dt><dd>Read changes and post reviews</dd></div></dl><form method="post" action="{}"><input type="hidden" name="manifest" value="{}"><button type="submit">Continue to GitHub</button></form>"#,
            escape_html(repo),
            escape_html(&action),
            escape_html(&serde_json::to_string(&config)?)
        ),
        &nonce,
        identity,
    );
    let url = format!("http://{host}{start}");
    eprintln!("Open {url} to approve App registration in GitHub");
    open_browser(&url);
    let deadline = Instant::now() + Duration::from_secs(900);
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Some(code) = handle_registration_request(
                    &mut stream,
                    &host,
                    &start,
                    &route,
                    &state,
                    &nonce,
                    &form,
                    identity,
                )? {
                    return github::api(
                        &format!("app-manifests/{code}/conversions"),
                        Some(&json!({})),
                        "POST",
                        false,
                    )?
                    .ok_or_else(|| anyhow!("App registration returned no credentials"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("App registration timed out; no repository settings were changed")
}

pub fn credentials(repo: &str, app: &Value, identity: Identity) -> Result<()> {
    require_app_owner(app)?;
    require_permissions(app)?;
    let client_id = app["client_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("App registration returned no Client ID"))?;
    let pem = app["pem"]
        .as_str()
        .filter(|value| value.contains("PRIVATE KEY-----"))
        .ok_or_else(|| anyhow!("App registration returned no private key"))?;
    let slug = app["slug"]
        .as_str()
        .filter(|value| Regex::new(r"^[a-z0-9-]+$").is_ok_and(|regex| regex.is_match(value)))
        .ok_or_else(|| anyhow!("App registration returned no valid slug"))?;
    save_credentials(repo, identity, client_id, pem, slug).with_context(|| {
        format!("could not store App credentials in {repo}; no private key was written locally")
    })?;
    let install = format!("https://github.com/apps/{slug}/installations/new");
    eprintln!("Install the App with Only select repositories -> {repo}");
    open_browser(&install);
    eprintln!("Press Enter after completing that installation in GitHub");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(())
}

fn save_credentials(
    repo: &str,
    identity: Identity,
    client_id: &str,
    pem: &str,
    slug: &str,
) -> Result<()> {
    let prefix = identity.prefix();
    let secret = format!("{prefix}_APP_PRIVATE_KEY");
    let client = format!("{prefix}_APP_CLIENT_ID");
    let slug_name = format!("{prefix}_APP_SLUG");
    for (arguments, data) in [
        (vec!["secret", "set", &secret, "--repo", repo], Some(pem)),
        (
            vec![
                "variable", "set", &client, "--repo", repo, "--body", client_id,
            ],
            None,
        ),
        (
            vec![
                "variable", "set", &slug_name, "--repo", repo, "--body", slug,
            ],
            None,
        ),
    ] {
        let arguments = arguments
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        github::gh(&arguments, data, false)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_registration_request(
    stream: &mut TcpStream,
    host: &str,
    start: &str,
    route: &str,
    state: &str,
    nonce: &str,
    form: &str,
    identity: Identity,
) -> Result<Option<String>> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut request = Vec::with_capacity(1_024);
    while request.len() < 16_384 && !request.windows(4).any(|ending| ending == b"\r\n\r\n") {
        let mut chunk = [0_u8; 1_024];
        let limit = chunk.len().min(16_384 - request.len());
        let length = stream.read(&mut chunk[..limit])?;
        if length == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..length]);
    }
    if !request.windows(4).any(|ending| ending == b"\r\n\r\n") {
        respond(stream, 400, "Bad request", nonce)?;
        return Ok(None);
    }
    let request = String::from_utf8_lossy(&request);
    let mut lines = request.lines();
    let target = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default();
    let valid_host = lines.any(|line| line.strip_prefix("Host: ") == Some(host));
    if !valid_host {
        respond(stream, 400, "Bad request", nonce)?;
        return Ok(None);
    }
    if target == DUCK_ROUTE {
        respond_duck(stream)?;
        return Ok(None);
    }
    if target == start {
        respond(stream, 200, form, nonce)?;
        return Ok(None);
    }
    match callback_code(target, route, state) {
        Ok(code) => {
            let body = setup_page(
                "App registered",
                "<p>Return to your terminal to finish setup.</p>",
                nonce,
                identity,
            );
            respond(stream, 200, &body, nonce)?;
            Ok(Some(code))
        }
        Err(_) => {
            respond(stream, 400, "Bad request", nonce)?;
            Ok(None)
        }
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: &str, nonce: &str) -> Result<()> {
    let label = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {label}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; img-src 'self'; style-src 'nonce-{nonce}'; form-action https://github.com; frame-ancestors 'none'; base-uri 'none'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    Ok(())
}

fn respond_duck(stream: &mut TcpStream) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        RADY_DUCK_PNG.len()
    )?;
    stream.write_all(RADY_DUCK_PNG)?;
    Ok(())
}

fn setup_page(title: &str, content: &str, nonce: &str, identity: Identity) -> String {
    let template = include_str!("dependasolver/templates/setup.html");
    template
        .replace("$title", &escape_html(title))
        .replace("$content", content)
        .replace("$nonce", &escape_html(nonce))
        .replace("$mascot", DUCK_ROUTE)
        .replace("$brand", identity.display())
}

fn random_token(bytes: usize) -> Result<String> {
    let mut data = vec![0_u8; bytes];
    getrandom::fill(&mut data)
        .map_err(|error| anyhow!("secure randomness is unavailable: {error}"))?;
    Ok(data.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| difference | left ^ right)
        == 0
}

fn percent_decode(value: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut source = value.as_bytes().iter().copied();
    while let Some(byte) = source.next() {
        if byte == b'%' {
            let high = source
                .next()
                .ok_or_else(|| anyhow!("invalid URL encoding"))?;
            let low = source
                .next()
                .ok_or_else(|| anyhow!("invalid URL encoding"))?;
            let hex = [high, low];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex)?, 16)?);
        } else if byte == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).context("callback query is not UTF-8")
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

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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
            let _ = std::process::Command::new(binary)
                .args(*arguments)
                .arg(url)
                .spawn();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_routes_serve_fragmented_page_and_duck() -> Result<()> {
        let state = random_token(32)?;
        let nonce = random_token(32)?;
        for (target, content_type, expected_body) in [
            ("/start", "text/html; charset=utf-8", b"Ready".as_slice()),
            (DUCK_ROUTE, "image/png", RADY_DUCK_PNG),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let address = listener.local_addr()?;
            let host = address.to_string();
            let request_host = host.clone();
            let browser = thread::spawn(move || -> Result<Vec<u8>> {
                let mut stream = TcpStream::connect(address)?;
                thread::sleep(Duration::from_millis(20));
                stream.write_all(format!("GET {target} HTTP/1.1\r\n").as_bytes())?;
                thread::sleep(Duration::from_millis(20));
                stream.write_all(format!("Host: {request_host}\r\n\r\n").as_bytes())?;
                let mut response = Vec::new();
                stream.read_to_end(&mut response)?;
                Ok(response)
            });
            let (mut stream, _) = listener.accept()?;
            stream.set_nonblocking(true)?;
            let result = handle_registration_request(
                &mut stream,
                &host,
                "/start",
                "/callback",
                &state,
                &nonce,
                "Ready",
                Identity::Rady,
            )?;
            assert!(result.is_none());
            drop(stream);
            let response = browser
                .join()
                .map_err(|_| anyhow!("browser thread panicked"))??;
            let body_start = response
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
                .ok_or_else(|| anyhow!("response has no header terminator"))?;
            let headers = std::str::from_utf8(&response[..body_start])?;
            assert!(headers.starts_with("HTTP/1.1 200 OK"), "{headers:?}");
            assert!(headers.contains(&format!("Content-Type: {content_type}")));
            assert_eq!(&response[body_start..], expected_body, "{target}");
        }
        Ok(())
    }

    #[test]
    fn callback_matrix_covers_valid_state_path_encoding_and_bad_codes() -> Result<()> {
        let route = "/callback/r";
        let state = random_token(32)?;
        for (path, valid) in [
            (
                format!("{route}?state={state}&code={}", "a".repeat(20)),
                true,
            ),
            (
                format!("/wrong?state={state}&code={}", "a".repeat(20)),
                false,
            ),
            (
                format!("{route}?state=wrong&code={}", "a".repeat(20)),
                false,
            ),
            (format!("{route}?state={state}&code=short"), false),
        ] {
            assert_eq!(callback_code(&path, route, &state).is_ok(), valid, "{path}");
        }
        Ok(())
    }

    #[test]
    fn permissions_are_read_only_except_for_replies_and_reviews() -> Result<()> {
        let mut app = json!({"permissions": permissions(), "owner": {"login": APP_OWNER}});
        assert_eq!(app["permissions"]["contents"], "read");
        require_app_owner(&app)?;
        require_permissions(&app)?;
        app["permissions"]["contents"] = json!("write");
        require_permissions(&app)?;
        app["permissions"]["contents"] = json!("none");
        assert!(require_permissions(&app).is_err());
        app["permissions"]["contents"] = json!("read");
        app["permissions"]["issues"] = json!("read");
        assert!(require_permissions(&app).is_err());
        app["permissions"]["issues"] = json!("write");
        app["permissions"]["pull_requests"] = json!("read");
        assert!(require_permissions(&app).is_err());
        Ok(())
    }

    #[test]
    fn manifest_uses_the_radyybot_name_and_product_description() {
        let manifest = manifest("keys-i/rady", "http://127.0.0.1/callback", Identity::Rady);
        assert_eq!(manifest["name"], "radyybot");
        assert_eq!(
            manifest["description"],
            "Rady reviews dependency updates from their diff and CI evidence, enables protected auto-merge when safe, and answers trusted @radyybot requests."
        );
    }

    #[test]
    fn setup_page_uses_the_animated_rady_duck() -> Result<()> {
        let page = setup_page(
            "Ready",
            "<p>Safe content</p>",
            &random_token(32)?,
            Identity::Rady,
        );
        assert!(page.contains("class=\"duck\""));
        assert!(page.contains("alt=\"Rady duck\""));
        assert!(page.contains("src=\"/rady-duck.png\""));
        assert!(page.contains("@keyframes duck-idle"));
        assert!(page.contains("@keyframes duck-arrive"));
        assert!(!page.contains("possum"));
        assert!(page.contains("prefers-reduced-motion"));
        assert!(page.contains("prefers-color-scheme: dark"));
        assert!(page.contains("forced-colors: active"));
        assert!(page.contains("aria-labelledby=\"setup-title\""));
        assert!(!page.contains("base64"));
        assert!(RADY_DUCK_PNG.starts_with(b"\x89PNG\r\n\x1a\n"));
        Ok(())
    }
}
