use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde_json::Value;

use crate::Result;
use crate::agent;
use crate::github::validate_repository;

const APP_API_BASE: &str = "https://api.github.com/";
const APP_API_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_STATUS_MARKER: &str = "\nRADY_HTTP_STATUS:";
const MAX_APP_API_RESPONSE_BYTES: usize = 1_000_000;
const MAX_APP_INSTALLATIONS: usize = 256;
const MAX_INSTALLATION_SCAN: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallationTokenScope {
    Mentions,
    Targets,
}

pub(crate) fn mint_installation_tokens(
    private_key_pem: &str,
    issuer: &str,
    owner: Option<&str>,
    scope: InstallationTokenScope,
    installation_seed: usize,
) -> Result<Vec<String>> {
    if let Some(owner) = owner {
        validate_repository(&format!("{owner}/rady"))?;
    }
    let key = app_signing_key(private_key_pem)?;
    let request = service_token_request(scope);
    app_installation_ids(&key, issuer, owner, installation_seed)?
        .into_iter()
        .map(|installation| {
            let jwt = current_app_jwt(&key, issuer)?;
            let endpoint = format!("app/installations/{installation}/access_tokens");
            let response = app_api(&endpoint, Some(&request), "POST", false, Some(&jwt))?
                .ok_or_else(|| anyhow!("GitHub returned no installation token"))?;
            installation_token(&response)
        })
        .collect()
}

pub(crate) fn authenticated_app(private_key_pem: &str, issuer: &str) -> Result<Value> {
    let key = app_signing_key(private_key_pem)?;
    let jwt = current_app_jwt(&key, issuer)?;
    app_api("app", None, "GET", false, Some(&jwt))?
        .ok_or_else(|| anyhow!("GitHub returned no App identity"))
}

pub(crate) fn public_app(slug: &str) -> Result<Value> {
    app_api(&format!("apps/{slug}"), None, "GET", false, None)?
        .ok_or_else(|| anyhow!("could not verify the existing public App"))
}

fn service_token_request(scope: InstallationTokenScope) -> Value {
    let permissions = match scope {
        InstallationTokenScope::Mentions => serde_json::json!({
            "administration": "read",
            "checks": "read",
            "contents": "read",
            "issues": "write",
            "pull_requests": "read",
            "statuses": "read"
        }),
        InstallationTokenScope::Targets => serde_json::json!({
            "administration": "read",
            "contents": "read",
            "issues": "read",
            "pull_requests": "read"
        }),
    };
    serde_json::json!({"permissions": permissions})
}

fn app_signing_key(private_key_pem: &str) -> Result<RsaKeyPair> {
    let (private_key, pkcs8) = decode_app_private_key(private_key_pem)?;
    if pkcs8 {
        RsaKeyPair::from_pkcs8(&private_key)
    } else {
        RsaKeyPair::from_der(&private_key)
    }
    .map_err(|_| anyhow!("GitHub App private key is not a valid RSA key"))
}

fn current_app_jwt(key: &RsaKeyPair, issuer: &str) -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    app_jwt(key, issuer, now)
}

fn app_jwt(key: &RsaKeyPair, issuer: &str, now: u64) -> Result<String> {
    let unsigned = unsigned_app_jwt(issuer, now)?;
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        unsigned.as_bytes(),
        &mut signature,
    )
    .map_err(|_| anyhow!("could not sign the GitHub App request"))?;
    Ok(format!("{unsigned}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

fn unsigned_app_jwt(issuer: &str, now: u64) -> Result<String> {
    if issuer.is_empty()
        || issuer.len() > 100
        || !issuer
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("GitHub App client ID is invalid");
    }
    let issued_at = now.saturating_sub(60);
    let expires_at = now
        .checked_add(540)
        .ok_or_else(|| anyhow!("system clock is outside the supported range"))?;
    let header = serde_json::to_vec(&serde_json::json!({"alg": "RS256", "typ": "JWT"}))?;
    let claims = serde_json::to_vec(&serde_json::json!({
        "iat": issued_at,
        "exp": expires_at,
        "iss": issuer,
    }))?;
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header),
        URL_SAFE_NO_PAD.encode(claims)
    ))
}

fn decode_app_private_key(pem: &str) -> Result<(Vec<u8>, bool)> {
    const MAX_PEM_BYTES: usize = 64 * 1024;
    if pem.is_empty() || pem.len() > MAX_PEM_BYTES || pem.as_bytes().contains(&0) {
        bail!("GitHub App private key is missing or too large");
    }
    for (label, pkcs8) in [("RSA PRIVATE KEY", false), ("PRIVATE KEY", true)] {
        let begin = format!("-----BEGIN {label}-----");
        let end = format!("-----END {label}-----");
        let value = pem.trim();
        if !value.starts_with(&begin) {
            continue;
        }
        let body = value
            .strip_prefix(&begin)
            .and_then(|value| value.strip_suffix(&end))
            .ok_or_else(|| anyhow!("GitHub App private key has an invalid PEM envelope"))?;
        if body.contains("-----") {
            bail!("GitHub App private key has an invalid PEM body");
        }
        let encoded = body
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        if encoded.is_empty() || encoded.len() > MAX_PEM_BYTES {
            bail!("GitHub App private key has an invalid PEM body");
        }
        let private_key = STANDARD
            .decode(encoded)
            .map_err(|_| anyhow!("GitHub App private key has invalid base64"))?;
        return Ok((private_key, pkcs8));
    }
    bail!("GitHub App private key must be an unencrypted RSA PEM")
}

fn app_installation_ids(
    key: &RsaKeyPair,
    issuer: &str,
    owner: Option<&str>,
    installation_seed: usize,
) -> Result<Vec<u64>> {
    if let Some(owner) = owner {
        for endpoint in [
            format!("users/{owner}/installation"),
            format!("orgs/{owner}/installation"),
        ] {
            let jwt = current_app_jwt(key, issuer)?;
            if let Some(response) = app_api(&endpoint, None, "GET", true, Some(&jwt))? {
                return installation_id(&response).map(|id| vec![id]);
            }
        }
        bail!("the GitHub App is not installed for {owner}");
    }
    let mut ids = Vec::with_capacity(MAX_INSTALLATION_SCAN);
    for page in 1..=(MAX_INSTALLATION_SCAN / 100 + 1) {
        let jwt = current_app_jwt(key, issuer)?;
        let response = app_api(
            &format!("app/installations?per_page=100&page={page}"),
            None,
            "GET",
            false,
            Some(&jwt),
        )?
        .ok_or_else(|| anyhow!("GitHub returned no App installations"))?;
        let complete = append_installation_page(&mut ids, &response)?;
        if ids.len() >= MAX_INSTALLATION_SCAN {
            eprintln!(
                "RadDuck found more than {MAX_INSTALLATION_SCAN} installations; rotating through the first {MAX_INSTALLATION_SCAN}"
            );
            break;
        }
        if complete {
            if ids.is_empty() {
                bail!("install the GitHub App before starting the service");
            }
            break;
        }
    }
    if ids.is_empty() {
        bail!("install the GitHub App before starting the service");
    }
    select_installations(&mut ids, installation_seed);
    Ok(ids)
}

fn select_installations(ids: &mut Vec<u64>, seed: usize) {
    ids.sort_unstable();
    ids.dedup();
    if ids.len() > MAX_APP_INSTALLATIONS {
        let offset = seed % ids.len();
        ids.rotate_left(offset);
        ids.truncate(MAX_APP_INSTALLATIONS);
    }
}

fn app_api(
    endpoint: &str,
    payload: Option<&Value>,
    method: &str,
    missing: bool,
    jwt: Option<&str>,
) -> Result<Option<Value>> {
    let curl = agent::which("curl").ok_or_else(|| anyhow!("install curl to connect RadDuck"))?;
    let payload = payload.map(serde_json::to_string).transpose()?;
    let arguments = app_api_arguments(method, endpoint, payload.as_deref(), jwt.is_some());
    let authorization = jwt.map(app_authorization).unwrap_or_default();
    let output = agent::execute(
        curl.as_os_str(),
        &arguments,
        Path::new("."),
        authorization.as_bytes(),
        APP_API_TIMEOUT + Duration::from_secs(5),
        &BTreeMap::new(),
        false,
        None,
    )?;
    let response = app_api_response(output.code, &output.stdout, missing).with_context(|| {
        format!(
            "GitHub API {method} {}",
            super::safe_endpoint_label(endpoint)
        )
    })?;
    response
        .map(|body| serde_json::from_str(&body).context("GitHub returned invalid JSON"))
        .transpose()
}

fn app_api_arguments(
    method: &str,
    endpoint: &str,
    payload: Option<&str>,
    authenticated: bool,
) -> Vec<String> {
    let mut arguments = vec![
        "--disable".to_owned(),
        "--silent".to_owned(),
        "--show-error".to_owned(),
        "--fail-with-body".to_owned(),
        "--proto".to_owned(),
        "=https".to_owned(),
        "--tlsv1.2".to_owned(),
        "--no-location".to_owned(),
        "--request".to_owned(),
        method.to_owned(),
        "--header".to_owned(),
        "Accept: application/vnd.github+json".to_owned(),
        "--header".to_owned(),
        "X-GitHub-Api-Version: 2022-11-28".to_owned(),
        "--header".to_owned(),
        "User-Agent: Rady".to_owned(),
    ];
    if authenticated {
        arguments.extend(["--header".to_owned(), "@-".to_owned()]);
    }
    if let Some(payload) = payload {
        arguments.extend([
            "--header".to_owned(),
            "Content-Type: application/json".to_owned(),
            "--data-raw".to_owned(),
            payload.to_owned(),
        ]);
    }
    arguments.extend([
        "--connect-timeout".to_owned(),
        "5".to_owned(),
        "--max-time".to_owned(),
        APP_API_TIMEOUT.as_secs().to_string(),
        "--max-filesize".to_owned(),
        MAX_APP_API_RESPONSE_BYTES.to_string(),
        "--write-out".to_owned(),
        format!("{HTTP_STATUS_MARKER}%{{http_code}}"),
        format!("{APP_API_BASE}{endpoint}"),
    ]);
    arguments
}

fn app_authorization(jwt: &str) -> String {
    format!("Authorization: Bearer {jwt}\n")
}

fn app_api_response(code: i32, output: &str, missing: bool) -> Result<Option<String>> {
    let (body, status) = output
        .rsplit_once(HTTP_STATUS_MARKER)
        .ok_or_else(|| anyhow!("GitHub returned no HTTP status"))?;
    let status = status
        .parse::<u16>()
        .map_err(|_| anyhow!("GitHub returned an invalid HTTP status"))?;
    if missing && status == 404 {
        return Ok(None);
    }
    if code != 0 || !(200..300).contains(&status) {
        let message = match (code, status) {
            (6, _) => "Rady couldn't resolve api.github.com".to_owned(),
            (7, _) => "Rady couldn't connect to GitHub".to_owned(),
            (28, _) => "GitHub took too long to respond".to_owned(),
            (63, _) => format!(
                "GitHub returned more than {MAX_APP_API_RESPONSE_BYTES} bytes; narrow the request"
            ),
            (_, 401) => "GitHub didn't accept RadDuck's App credentials (401); check that the client ID and private key belong to the same App".to_owned(),
            (_, 403) => "GitHub wouldn't allow this App request (403); check RadDuck's permissions and installation".to_owned(),
            (_, 404) => "GitHub couldn't find this App resource (404); check the RadDuck installation".to_owned(),
            (_, 429) => "GitHub's rate limit is full (429); try again after it resets".to_owned(),
            _ if status != 0 => format!("GitHub rejected the App request (HTTP {status})"),
            _ => format!("Rady couldn't reach GitHub (transport {code})"),
        };
        bail!(message);
    }
    if body.len() > MAX_APP_API_RESPONSE_BYTES {
        bail!("GitHub returned too much data; narrow the request");
    }
    Ok((!body.trim().is_empty()).then(|| body.to_owned()))
}

fn append_installation_page(ids: &mut Vec<u64>, response: &Value) -> Result<bool> {
    let installations = response
        .as_array()
        .ok_or_else(|| anyhow!("GitHub returned invalid App installations"))?;
    for installation in installations {
        if ids.len() == MAX_INSTALLATION_SCAN {
            return Ok(false);
        }
        ids.push(installation_id(installation)?);
    }
    Ok(installations.len() < 100)
}

fn installation_id(response: &Value) -> Result<u64> {
    response["id"]
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or_else(|| anyhow!("GitHub returned an invalid App installation"))
}

fn installation_token(response: &Value) -> Result<String> {
    response["token"]
        .as_str()
        .filter(|token| {
            !token.is_empty()
                && token.len() <= 8_192
                && token.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("GitHub returned an invalid installation token"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_jwt_contract_is_compact_and_time_bounded() -> Result<()> {
        let unsigned = unsigned_app_jwt("Iv1.123abc", 1_000)?;
        let parts = unsigned.split('.').collect::<Vec<_>>();
        assert_eq!(parts.len(), 2);
        let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0])?)?;
        let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1])?)?;
        assert_eq!(header, serde_json::json!({"alg": "RS256", "typ": "JWT"}));
        assert_eq!(claims["iss"], "Iv1.123abc");
        assert_eq!(claims["iat"], 940);
        assert_eq!(claims["exp"], 1_540);

        for issuer in ["", "not valid", "client/id", &"a".repeat(101)] {
            assert!(unsigned_app_jwt(issuer, 1_000).is_err(), "{issuer}");
        }
        Ok(())
    }

    #[test]
    fn app_jwt_signs_with_a_real_ephemeral_rsa_key() -> Result<()> {
        let openssl = crate::agent::which("openssl")
            .ok_or_else(|| anyhow!("openssl is required for the RSA integration test"))?;
        let temporary = tempfile::tempdir()?;
        let private_key = temporary.path().join("test-private-key.pem");
        let output = std::process::Command::new(openssl)
            .args([
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:2048",
            ])
            .arg("-out")
            .arg(&private_key)
            .output()?;
        if !output.status.success() {
            bail!("openssl could not create the ephemeral RSA test key");
        }

        let key = app_signing_key(&std::fs::read_to_string(private_key)?)?;
        let jwt = app_jwt(&key, "Iv1.test", 1_000)?;
        let (unsigned, encoded_signature) = jwt
            .rsplit_once('.')
            .ok_or_else(|| anyhow!("signed JWT omitted its signature"))?;
        let signature = URL_SAFE_NO_PAD.decode(encoded_signature)?;
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            key.public().as_ref(),
        )
        .verify(unsigned.as_bytes(), &signature)
        .map_err(|_| anyhow!("signed JWT did not verify"))?;
        Ok(())
    }

    #[test]
    fn app_credentials_and_api_values_fail_closed() -> Result<()> {
        let mentions = service_token_request(InstallationTokenScope::Mentions);
        assert_eq!(mentions["permissions"]["issues"], "write");
        assert_eq!(mentions["permissions"]["checks"], "read");
        assert_eq!(mentions["permissions"]["contents"], "read");
        assert_eq!(
            mentions["permissions"].as_object().map(|value| value.len()),
            Some(6)
        );
        let targets = service_token_request(InstallationTokenScope::Targets);
        assert_eq!(targets["permissions"]["issues"], "read");
        assert_eq!(targets["permissions"]["contents"], "read");
        assert!(targets["permissions"].get("checks").is_none());
        assert_eq!(
            targets["permissions"].as_object().map(|value| value.len()),
            Some(4)
        );

        for (pem, expected) in [
            (
                "-----BEGIN RSA PRIVATE KEY-----\nAQID\n-----END RSA PRIVATE KEY-----",
                Some(false),
            ),
            (
                "-----BEGIN PRIVATE KEY-----\nAQID\n-----END PRIVATE KEY-----",
                Some(true),
            ),
            ("", None),
            (
                "-----BEGIN PRIVATE KEY-----\n%%%\n-----END PRIVATE KEY-----",
                None,
            ),
            (
                "-----BEGIN ENCRYPTED PRIVATE KEY-----\nAQID\n-----END ENCRYPTED PRIVATE KEY-----",
                None,
            ),
        ] {
            let decoded = decode_app_private_key(pem);
            assert_eq!(decoded.as_ref().ok().map(|(_, pkcs8)| *pkcs8), expected);
        }

        for (response, valid) in [
            (serde_json::json!({"id": 42}), true),
            (serde_json::json!({"id": 0}), false),
            (serde_json::json!({"id": "42"}), false),
        ] {
            assert_eq!(installation_id(&response).is_ok(), valid, "{response}");
        }
        for (response, valid) in [
            (serde_json::json!({"token": "ghs_valid"}), true),
            (serde_json::json!({"token": ""}), false),
            (serde_json::json!({"token": "bad token"}), false),
            (serde_json::json!({}), false),
        ] {
            assert_eq!(installation_token(&response).is_ok(), valid, "{response}");
        }

        let mut ids = Vec::new();
        assert!(append_installation_page(
            &mut ids,
            &serde_json::json!([{"id": 2}, {"id": 1}])
        )?);
        assert_eq!(ids, [2, 1]);
        let oversized = Value::Array(
            (1..=MAX_APP_INSTALLATIONS + 1)
                .map(|id| serde_json::json!({"id": id}))
                .collect(),
        );
        let mut bounded = Vec::new();
        assert!(!append_installation_page(&mut bounded, &oversized)?);
        assert_eq!(bounded.len(), MAX_APP_INSTALLATIONS + 1);
        select_installations(&mut bounded, 0);
        assert_eq!(bounded.len(), MAX_APP_INSTALLATIONS);
        Ok(())
    }

    #[test]
    fn installation_selection_is_bounded_and_rotates() {
        let mut ids = (1..=300).rev().collect::<Vec<_>>();
        select_installations(&mut ids, 299);
        assert_eq!(ids.len(), MAX_APP_INSTALLATIONS);
        assert_eq!(ids[0], 300);
        assert_eq!(ids[1], 1);
    }

    #[test]
    fn app_transport_uses_bearer_without_exposing_the_jwt() {
        let jwt = "header.payload.signature";
        assert_eq!(
            app_authorization(jwt),
            "Authorization: Bearer header.payload.signature\n"
        );
        for (method, payload, sends_body) in [
            ("GET", None, false),
            ("POST", Some(r#"{"permissions":{"contents":"read"}}"#), true),
        ] {
            let arguments = app_api_arguments(method, "app/installations", payload, true);
            assert!(
                arguments.windows(2).any(|pair| pair == ["--header", "@-"]),
                "{arguments:?}"
            );
            assert!(!arguments.iter().any(|argument| argument.contains(jwt)));
            assert_eq!(
                arguments.iter().any(|argument| argument == "--data-raw"),
                sends_body
            );
        }

        let public_arguments = app_api_arguments("GET", "apps/radduck", None, false);
        assert!(
            !public_arguments
                .windows(2)
                .any(|pair| pair == ["--header", "@-"])
        );
        assert_eq!(
            public_arguments.last().map(String::as_str),
            Some("https://api.github.com/apps/radduck")
        );

        for (code, response, missing, expected) in [
            (0, "[]\nRADY_HTTP_STATUS:200", false, Some("[]")),
            (22, "hidden\nRADY_HTTP_STATUS:404", true, None),
        ] {
            assert_eq!(
                app_api_response(code, response, missing)
                    .unwrap()
                    .as_deref(),
                expected
            );
        }
        let error = app_api_response(22, "hidden\nRADY_HTTP_STATUS:401", false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("didn't accept RadDuck's App credentials"));
        assert!(!error.contains("hidden"));
    }
}
