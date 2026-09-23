use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde_json::Value;

use crate::Result;
use crate::github::{api_authenticated, validate_repository};

pub(crate) fn mint_installation_tokens(
    private_key_pem: &str,
    issuer: &str,
    owner: Option<&str>,
) -> Result<Vec<String>> {
    if let Some(owner) = owner {
        validate_repository(&format!("{owner}/rady"))?;
    }
    let key = app_signing_key(private_key_pem)?;
    let request = service_token_request();
    app_installation_ids(&key, issuer, owner)?
        .into_iter()
        .map(|installation| {
            let jwt = current_app_jwt(&key, issuer)?;
            let endpoint = format!("app/installations/{installation}/access_tokens");
            let response = api_authenticated(&endpoint, Some(&request), "POST", false, &jwt)?
                .ok_or_else(|| anyhow!("GitHub returned no installation token"))?;
            installation_token(&response)
        })
        .collect()
}

fn service_token_request() -> Value {
    serde_json::json!({
        "permissions": {
            "administration": "read",
            "checks": "read",
            "contents": "read",
            "issues": "write",
            "pull_requests": "write",
            "statuses": "read"
        }
    })
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
        bail!("GitHub App client ID or App ID is invalid");
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

const MAX_APP_INSTALLATIONS: usize = 256;

fn app_installation_ids(key: &RsaKeyPair, issuer: &str, owner: Option<&str>) -> Result<Vec<u64>> {
    if let Some(owner) = owner {
        for endpoint in [
            format!("users/{owner}/installation"),
            format!("orgs/{owner}/installation"),
        ] {
            let jwt = current_app_jwt(key, issuer)?;
            if let Some(response) = api_authenticated(&endpoint, None, "GET", true, &jwt)? {
                return installation_id(&response).map(|id| vec![id]);
            }
        }
        bail!("the GitHub App is not installed for {owner}");
    }
    let mut ids = Vec::new();
    for page in 1..=3 {
        let jwt = current_app_jwt(key, issuer)?;
        let response = api_authenticated(
            &format!("app/installations?per_page=100&page={page}"),
            None,
            "GET",
            false,
            &jwt,
        )?
        .ok_or_else(|| anyhow!("GitHub returned no App installations"))?;
        if append_installation_page(&mut ids, &response)? {
            if ids.is_empty() {
                bail!("install the GitHub App before starting the service");
            }
            return Ok(ids);
        }
    }
    bail!("GitHub App has too many installations; shard the service with --owner")
}

fn append_installation_page(ids: &mut Vec<u64>, response: &Value) -> Result<bool> {
    let installations = response
        .as_array()
        .ok_or_else(|| anyhow!("GitHub returned invalid App installations"))?;
    for installation in installations {
        if ids.len() == MAX_APP_INSTALLATIONS {
            bail!("GitHub App has too many installations; shard the service with --owner");
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
        let request = service_token_request();
        assert_eq!(request["permissions"]["contents"], "read");
        assert_eq!(request["permissions"]["issues"], "write");
        assert_eq!(request["permissions"]["pull_requests"], "write");
        assert_eq!(
            request["permissions"].as_object().map(|value| value.len()),
            Some(6)
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
        assert!(append_installation_page(&mut Vec::new(), &oversized).is_err());
        Ok(())
    }
}
