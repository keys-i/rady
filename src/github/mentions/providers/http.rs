use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

use tempfile::tempdir;

use super::{FailureKind, ProviderFailure};
use crate::agent;

pub(super) const MAX_RESPONSE_BYTES: usize = 24_000;
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(20);
const HTTP_STATUS_MARKER: &str = "\nPEKIN_HTTP_STATUS:";
const RETRY_AFTER_MARKER: &str = "\nPEKIN_RETRY_AFTER:";

pub(super) fn get(
    url: &str,
    header_name: &str,
    header_value: &str,
) -> Result<String, ProviderFailure> {
    call(url, header_name, header_value, "GET", None)
}

pub(super) fn post(
    url: &str,
    header_name: &str,
    header_value: &str,
    body: Vec<u8>,
) -> Result<String, ProviderFailure> {
    call(url, header_name, header_value, "POST", Some(body))
}

fn call(
    url: &str,
    header_name: &str,
    header_value: &str,
    method: &str,
    body: Option<Vec<u8>>,
) -> Result<String, ProviderFailure> {
    let curl = agent::which("curl").ok_or_else(|| {
        ProviderFailure::new(FailureKind::Transient, "curl is required for hosted models")
    })?;
    let directory = tempdir().map_err(|_| {
        ProviderFailure::new(
            FailureKind::Transient,
            "could not create a private request directory",
        )
    })?;
    let config = directory.path().join("curl.conf");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&config)
        .map_err(|_| {
            ProviderFailure::new(
                FailureKind::Transient,
                "could not prepare a private provider request",
            )
        })?;
    writeln!(
        file,
        "header = \"{}: {}\"",
        header_name,
        curl_escape(header_value)
    )
    .map_err(|_| {
        ProviderFailure::new(
            FailureKind::Transient,
            "could not prepare a private provider request",
        )
    })?;
    let arguments = provider_arguments(method, &config, url, body.is_some());
    let output = agent::execute(
        curl.as_os_str(),
        &arguments,
        directory.path(),
        body.as_deref().unwrap_or_default(),
        PROVIDER_TIMEOUT + Duration::from_secs(5),
        &BTreeMap::new(),
        false,
        None,
    )
    .map_err(|_| {
        ProviderFailure::new(
            FailureKind::Transient,
            "request failed before the provider responded",
        )
    })?;
    parse_response(output.code, &output.stdout)
}

fn provider_arguments(method: &str, config: &Path, url: &str, has_body: bool) -> Vec<String> {
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
        "--config".to_owned(),
        config.to_string_lossy().into_owned(),
    ];
    if has_body {
        arguments.extend([
            "--header".to_owned(),
            "Content-Type: application/json".to_owned(),
            "--data-binary".to_owned(),
            "@-".to_owned(),
        ]);
    }
    arguments.extend([
        "--connect-timeout".to_owned(),
        "5".to_owned(),
        "--max-time".to_owned(),
        PROVIDER_TIMEOUT.as_secs().to_string(),
        "--max-filesize".to_owned(),
        MAX_RESPONSE_BYTES.to_string(),
        "--write-out".to_owned(),
        format!("{HTTP_STATUS_MARKER}%{{http_code}}{RETRY_AFTER_MARKER}%header{{retry-after}}"),
        url.to_owned(),
    ]);
    arguments
}

fn parse_response(code: i32, output: &str) -> Result<String, ProviderFailure> {
    let Some((response, retry_after)) = output.rsplit_once(RETRY_AFTER_MARKER) else {
        return Err(transport_failure(code));
    };
    let Some((body, status)) = response.rsplit_once(HTTP_STATUS_MARKER) else {
        return Err(transport_failure(code));
    };
    let retry_after = retry_after
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs);
    let status = status.parse::<u16>().map_err(|_| {
        ProviderFailure::new(
            FailureKind::Transient,
            "provider returned an invalid HTTP status",
        )
    })?;
    if !(200..300).contains(&status) || code != 0 {
        return Err(status_failure(code, status, retry_after));
    }
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(ProviderFailure::new(
            FailureKind::InvalidResponse,
            format!("response exceeded {MAX_RESPONSE_BYTES} bytes"),
        ));
    }
    Ok(body.to_owned())
}

fn transport_failure(code: i32) -> ProviderFailure {
    let detail = match code {
        6 => "could not resolve the provider",
        7 => "could not connect to the provider",
        28 => "request timed out",
        63 => "response exceeded the size limit",
        _ => "provider request failed",
    };
    ProviderFailure::new(FailureKind::Transient, detail)
}

fn status_failure(code: i32, status: u16, retry_after: Option<Duration>) -> ProviderFailure {
    let (kind, detail) = match status {
        400 | 404 | 405 | 409 | 422 => (
            FailureKind::Model,
            format!("request was rejected (HTTP {status})"),
        ),
        401 | 403 => (
            FailureKind::Authentication,
            format!("authentication was rejected (HTTP {status})"),
        ),
        402 => (
            FailureKind::Payment,
            "provider reported payment or entitlement required (HTTP 402)".to_owned(),
        ),
        408 | 500..=599 => (
            FailureKind::Transient,
            format!("provider is temporarily unavailable (HTTP {status})"),
        ),
        429 => (
            FailureKind::RateLimit,
            "provider rate limit reached (HTTP 429)".to_owned(),
        ),
        _ if code != 0 => (
            FailureKind::Transient,
            format!("provider request failed (transport {code})"),
        ),
        _ => (
            FailureKind::Model,
            format!("request was rejected (HTTP {status})"),
        ),
    };
    ProviderFailure::new(kind, detail).retry_after(retry_after)
}

fn curl_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_bounded_http_results_without_exposing_bodies() {
        for (code, output, expected_kind, expected_text) in [
            (
                0,
                "{\"answer\":\"ready\"}\nPEKIN_HTTP_STATUS:200\nPEKIN_RETRY_AFTER:",
                None,
                None,
            ),
            (
                22,
                "secret provider body\nPEKIN_HTTP_STATUS:402\nPEKIN_RETRY_AFTER:",
                Some(FailureKind::Payment),
                Some("provider reported payment or entitlement required (HTTP 402)"),
            ),
            (
                22,
                "secret provider body\nPEKIN_HTTP_STATUS:429\nPEKIN_RETRY_AFTER:30",
                Some(FailureKind::RateLimit),
                Some("provider rate limit reached (HTTP 429)"),
            ),
            (
                28,
                "\nPEKIN_HTTP_STATUS:000\nPEKIN_RETRY_AFTER:",
                Some(FailureKind::Transient),
                Some("provider request failed (transport 28)"),
            ),
        ] {
            let result = parse_response(code, output);
            match expected_kind {
                Some(kind) => {
                    let error = result.unwrap_err();
                    assert_eq!(error.kind, kind);
                    assert_eq!(error.to_string(), expected_text.unwrap());
                    assert!(!error.to_string().contains("secret provider body"));
                }
                None => assert_eq!(result.unwrap(), r#"{"answer":"ready"}"#),
            }
        }
        let limited =
            parse_response(22, "\nPEKIN_HTTP_STATUS:429\nPEKIN_RETRY_AFTER:45").unwrap_err();
        assert_eq!(limited.retry_after, Some(Duration::from_secs(45)));
    }

    #[test]
    fn curl_stays_on_fixed_https_requests() {
        for (method, body, expected_body) in [("GET", false, false), ("POST", true, true)] {
            let arguments = provider_arguments(
                method,
                Path::new("/tmp/pekin-curl.conf"),
                "https://provider.example/v1/models",
                body,
            );
            assert!(
                arguments
                    .windows(2)
                    .any(|pair| pair == ["--proto", "=https"])
            );
            assert!(arguments.iter().any(|argument| argument == "--no-location"));
            assert_eq!(
                arguments.iter().any(|argument| argument == "--data-binary"),
                expected_body
            );
        }
    }
}
