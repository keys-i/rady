use std::collections::BTreeMap;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::tempdir;

use super::routing::{Intent, Tier};
use crate::agent;

const LAYA_URL: &str = "http://127.0.0.1:8000/v1/systemone";
const MAX_STATE_BYTES: usize = 3_000;
const MAX_REQUEST_BYTES: usize = 20_000;
const MAX_RESPONSE_BYTES: usize = 8_000;
const MIN_CONFIDENCE: f64 = 0.90;
const TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn classify_intent(request: &str) -> Option<Intent> {
    let choice = decide(
        &clipped(request, MAX_STATE_BYTES),
        "intent",
        "Choose the minimum execution context this request needs",
        json!({
            "read_only": "explain, inspect, compare, review, answer, or report without changing files or external state",
            "write": "edit files, run commands with effects, commit, publish, deploy, or change external state"
        }),
    )?;
    match choice.as_str() {
        "read_only" => Some(Intent::ReadOnly),
        "write" => Some(Intent::Write),
        _ => None,
    }
}

pub(super) fn select_tier(evidence: &Value) -> Option<Tier> {
    let evidence = decision_text(evidence);
    if evidence.is_empty() {
        return None;
    }
    let choice = decide(
        &evidence,
        "tier",
        "Choose the least model capability that can answer accurately and safely",
        json!({
            "fast": "short, direct, low-risk classification or factual response",
            "balanced": "normal code review, multi-part explanation, or moderate evidence synthesis",
            "deep": "security, failed checks, conflicts, incomplete evidence, broad changes, or subtle reasoning"
        }),
    )?;
    match choice.as_str() {
        "fast" => Some(Tier::Fast),
        "balanced" => Some(Tier::Balanced),
        "deep" => Some(Tier::Deep),
        _ => None,
    }
}

fn decide(state: &str, name: &str, instructions: &str, criteria: Value) -> Option<String> {
    if env::var("PEKIN_LAYA_ENABLED").as_deref() != Ok("true") {
        return None;
    }
    let body = serde_json::to_vec(&json!({
        "state": {"request": state},
        "questions": {
            name: {
                "type": "choice",
                "instructions": instructions,
                "criteria": criteria
            }
        },
        "model": "typed-decisions"
    }))
    .ok()?;
    if body.len() > MAX_REQUEST_BYTES {
        return None;
    }
    let output = call(&body)?;
    parse_decision(&output, name)
}

fn call(body: &[u8]) -> Option<Value> {
    let curl = agent::which("curl")?;
    let directory = tempdir().ok()?;
    let mut arguments = curl_arguments();
    let key = local_api_key(env::var("PEKIN_LAYA_API_KEY").ok())?;
    let config = directory.path().join("curl.conf");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&config)
        .ok()?;
    writeln!(
        file,
        "header = \"Authorization: Bearer {}\"",
        curl_escape(&key)
    )
    .ok()?;
    arguments.extend(["--config".to_owned(), config.to_string_lossy().into_owned()]);
    arguments.push(LAYA_URL.to_owned());
    let output = agent::execute(
        curl.as_os_str(),
        &arguments,
        directory.path(),
        body,
        TIMEOUT + Duration::from_secs(1),
        &BTreeMap::new(),
        false,
        None,
    )
    .ok()?;
    if output.code != 0 || output.stdout.len() > MAX_RESPONSE_BYTES {
        return None;
    }
    serde_json::from_str(&output.stdout).ok()
}

fn curl_arguments() -> Vec<String> {
    vec![
        "--disable".to_owned(),
        "--silent".to_owned(),
        "--show-error".to_owned(),
        "--fail-with-body".to_owned(),
        "--proto".to_owned(),
        "=http".to_owned(),
        "--no-location".to_owned(),
        "--noproxy".to_owned(),
        "*".to_owned(),
        "--request".to_owned(),
        "POST".to_owned(),
        "--header".to_owned(),
        "Content-Type: application/json".to_owned(),
        "--data-binary".to_owned(),
        "@-".to_owned(),
        "--connect-timeout".to_owned(),
        "1".to_owned(),
        "--max-time".to_owned(),
        TIMEOUT.as_secs().to_string(),
        "--max-filesize".to_owned(),
        MAX_RESPONSE_BYTES.to_string(),
    ]
}

fn parse_decision(value: &Value, name: &str) -> Option<String> {
    let answer = value.get("answers")?.get(name)?;
    let confidence = answer
        .get("answer_confidence")
        .and_then(Value::as_f64)
        .or_else(|| answer.get("confidence").and_then(Value::as_f64))?;
    if !(MIN_CONFIDENCE..=1.0).contains(&confidence) {
        return None;
    }
    answer.get("choice")?.as_str().map(str::to_owned)
}

fn decision_text(evidence: &Value) -> String {
    let mut text = String::new();
    for scope in std::iter::once(evidence)
        .chain(evidence.get("issue"))
        .chain(evidence.get("pull_request"))
    {
        for name in ["task", "request", "title", "description", "body"] {
            let Some(value) = scope.get(name).and_then(Value::as_str) else {
                continue;
            };
            let prefix = name.len() + 2 + usize::from(!text.is_empty());
            if text.len().saturating_add(prefix) >= MAX_STATE_BYTES {
                return text;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(name);
            text.push_str(": ");
            let remaining = MAX_STATE_BYTES.saturating_sub(text.len());
            text.push_str(&clipped(value, remaining));
            if text.len() >= MAX_STATE_BYTES {
                return text;
            }
        }
    }
    text
}

fn clipped(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn valid_api_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1_024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"' && byte != b'\\')
}

fn local_api_key(value: Option<String>) -> Option<String> {
    value.filter(|key| valid_api_key(key))
}

fn curl_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_confident_bounded_choices() {
        for (value, expected) in [
            (
                json!({"answers": {"intent": {"choice": "write", "answer_confidence": 0.97}}}),
                Some("write"),
            ),
            (
                json!({"answers": {"intent": {"choice": "read_only", "confidence": 0.90}}}),
                Some("read_only"),
            ),
            (
                json!({"answers": {"intent": {"choice": "write", "confidence": 0.89}}}),
                None,
            ),
            (json!({"answers": {"intent": {"choice": "write"}}}), None),
        ] {
            assert_eq!(parse_decision(&value, "intent").as_deref(), expected);
        }
        assert_eq!(clipped("duck🦆tail", 7), "duck");
        assert_eq!(
            decision_text(&json!({"request": "review this", "ignored": "secret"})),
            "request: review this"
        );
        assert!(valid_api_key("local-secret"));
        assert!(!valid_api_key("bad key"));
        assert_eq!(local_api_key(None), None);
        assert_eq!(local_api_key(Some(String::new())), None);
        assert_eq!(
            local_api_key(Some("local-secret".to_owned())).as_deref(),
            Some("local-secret")
        );
    }

    #[test]
    fn laya_curl_bypasses_inherited_proxies() {
        assert!(
            curl_arguments()
                .windows(2)
                .any(|arguments| arguments == ["--noproxy", "*"])
        );
    }
}
