use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail};
use serde_json::{Value, json};
use tempfile::tempdir;

use super::{MAX_ANSWER, MAX_EVIDENCE_BYTES};
use crate::Result;
use crate::agent;
use crate::routing::Tier;

const MAX_PROVIDER_RESPONSE_BYTES: usize = 24_000;
const MAX_CATALOG_MODELS: usize = 16;
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(20);
const HTTP_STATUS_MARKER: &str = "\nRADY_HTTP_STATUS:";

#[derive(Clone, Debug, Eq, PartialEq)]
enum HostedModel {
    Gemini(String),
    Cerebras(String),
    Xai(String),
}

impl HostedModel {
    const fn provider(&self) -> &'static str {
        match self {
            Self::Gemini(_) => "gemini",
            Self::Cerebras(_) => "cerebras",
            Self::Xai(_) => "xai",
        }
    }

    fn label(self) -> String {
        match self {
            Self::Gemini(model) => format!("Gemini {model}"),
            Self::Cerebras(model) => format!("Cerebras {model}"),
            Self::Xai(model) => format!("xAI {model}"),
        }
    }

    fn id(&self) -> &str {
        match self {
            Self::Gemini(model) | Self::Cerebras(model) | Self::Xai(model) => model,
        }
    }
}

pub(crate) fn hosted_json_answer(
    evidence: &str,
    instructions: &str,
    schema: &Value,
    tier: Tier,
    repository_private: Option<bool>,
) -> Result<Value> {
    if serde_json::to_vec(schema)?.len() > 16_000 {
        bail!("hosted response schema is too large");
    }
    let gemini_allowed = external_provider_permitted(
        repository_private,
        bool_environment("RADY_GEMINI_PRIVATE_OK") == Some(true),
    );
    let xai_allowed = external_provider_permitted(
        repository_private,
        bool_environment("RADY_XAI_PRIVATE_OK") == Some(true),
    );
    let cerebras_allowed = external_provider_permitted(
        repository_private,
        bool_environment("RADY_CEREBRAS_PRIVATE_OK") == Some(true),
    );
    let models = hosted_models(tier, gemini_allowed, cerebras_allowed, xai_allowed);
    let prompt = provider_prompt(evidence)?;
    match chained_request(&models, &prompt, instructions, schema, tier) {
        Ok(answer) => Ok(answer),
        Err(primary) => {
            let mut discovered = models.clone();
            discovered_models(
                tier,
                &mut discovered,
                gemini_allowed,
                cerebras_allowed,
                xai_allowed,
            );
            if discovered == models {
                return Err(primary);
            }
            chained_request(&discovered, &prompt, instructions, schema, tier).map_err(|fallback| {
                anyhow!("known models failed: {primary}; catalog fallback failed: {fallback}")
            })
        }
    }
}

fn chained_request(
    models: &[HostedModel],
    prompt: &str,
    instructions: &str,
    schema: &Value,
    tier: Tier,
) -> Result<Value> {
    let mut prompt = prompt.to_owned();
    let mut models = models.to_vec();
    if tier == Tier::Deep {
        let mut scouts = models.clone();
        scouts.sort_by_key(|model| model_rank(model.id(), Tier::Fast));
        let brief_schema = json!({
            "type": "object", "additionalProperties": false,
            "properties": {"brief": {"type": "string", "minLength": 1, "maxLength": 4000}},
            "required": ["brief"]
        });
        if let Ok((brief, used)) = request_models(
            &scouts,
            &prompt,
            "Produce a concise evidence brief of concrete facts, uncertainties and decisions for another model. Do not provide private chain-of-thought. Return only JSON matching the schema.",
            &brief_schema,
        ) {
            if let Some(brief) = brief["brief"].as_str() {
                prompt.push_str("\n\nIndependent evidence brief:\n");
                prompt.extend(brief.chars().take(4_000));
                models.sort_by_key(|model| model == &used);
            }
        }
    }
    request_models(&models, &prompt, instructions, schema).map(|(answer, _)| answer)
}

fn request_models(
    models: &[HostedModel],
    prompt: &str,
    instructions: &str,
    schema: &Value,
) -> Result<(Value, HostedModel)> {
    let mut failures = Vec::with_capacity(models.len());
    let mut unavailable = BTreeSet::new();
    for model in models.iter().cloned() {
        let provider = model.provider();
        if unavailable.contains(provider) {
            continue;
        }
        let key_name = match &model {
            HostedModel::Gemini(_) => "RADY_GEMINI_API_KEY",
            HostedModel::Cerebras(_) => "RADY_CEREBRAS_API_KEY",
            HostedModel::Xai(_) => "RADY_XAI_API_KEY",
        };
        let Some(key) = env::var(key_name).ok().filter(|key| !key.is_empty()) else {
            continue;
        };
        if !valid_api_key(&key) {
            bail!("{key_name} is invalid");
        }
        let result = match &model {
            HostedModel::Gemini(model) => gemini_answer(prompt, model, &key, instructions, schema),
            HostedModel::Cerebras(model) => {
                cerebras_answer(prompt, model, &key, instructions, schema)
            }
            HostedModel::Xai(model) => xai_answer(prompt, model, &key, instructions, schema),
        };
        match result {
            Ok(answer) => return Ok((answer, model)),
            Err(error) => {
                let message = error.to_string();
                if ["HTTP 401", "HTTP 402", "HTTP 403", "HTTP 429", "HTTP 503"]
                    .iter()
                    .any(|status| message.contains(status))
                {
                    unavailable.insert(provider);
                }
                failures.push(format!("{}: {message}", model.label()));
            }
        }
    }
    if !failures.is_empty() {
        bail!(
            "hosted model response failed: {}; no answer was posted",
            failures.join("; ")
        );
    }
    bail!("no permitted hosted model key is available and Codex is not signed in")
}

pub(super) fn answer_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "answer": {"type": "string", "minLength": 1, "maxLength": MAX_ANSWER},
            "follow_ups": {
                "type": "array", "maxItems": 3,
                "items": {"type": "string", "minLength": 1, "maxLength": 240}
            }
        },
        "required": ["answer"]
    })
}

pub(super) fn bool_environment(name: &str) -> Option<bool> {
    match env::var(name).ok()?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn external_provider_permitted(repository_private: Option<bool>, private_opt_in: bool) -> bool {
    repository_private == Some(false) || private_opt_in
}

fn hosted_models(
    tier: Tier,
    gemini_allowed: bool,
    cerebras_allowed: bool,
    xai_allowed: bool,
) -> Vec<HostedModel> {
    let mut models = Vec::with_capacity(4 + MAX_CATALOG_MODELS * 3);
    if gemini_allowed {
        models.push(HostedModel::Gemini(
            match tier {
                Tier::Fast => "gemini-3.5-flash-lite",
                Tier::Balanced | Tier::Deep => "gemini-3.8-flash",
            }
            .to_owned(),
        ));
    }
    if cerebras_allowed {
        if tier != Tier::Fast {
            models.push(HostedModel::Cerebras("qwen-3.8-27b".to_owned()));
        }
        models.push(HostedModel::Cerebras("gpt-oss-120b".to_owned()));
    }
    if xai_allowed {
        models.push(HostedModel::Xai("grok-4.7".to_owned()));
    }
    models
}

fn discovered_models(
    tier: Tier,
    models: &mut Vec<HostedModel>,
    gemini_allowed: bool,
    cerebras_allowed: bool,
    xai_allowed: bool,
) {
    let fallback = std::mem::take(models);
    let mut known = BTreeSet::new();
    for (provider, permitted, key_name, discover) in [
        (
            "gemini",
            gemini_allowed,
            "RADY_GEMINI_API_KEY",
            gemini_catalog as fn(&str) -> Result<Vec<String>>,
        ),
        (
            "cerebras",
            cerebras_allowed,
            "RADY_CEREBRAS_API_KEY",
            cerebras_catalog,
        ),
        ("xai", xai_allowed, "RADY_XAI_API_KEY", xai_catalog),
    ] {
        let discovered = permitted
            .then(|| env::var(key_name).ok().filter(|key| valid_api_key(key)))
            .flatten()
            .and_then(|key| discover(&key).ok());
        if let Some(ids) = discovered {
            for id in rank_catalog_models(ids, tier) {
                if known.insert((provider, id.clone())) {
                    models.push(match provider {
                        "gemini" => HostedModel::Gemini(id),
                        "cerebras" => HostedModel::Cerebras(id),
                        "xai" => HostedModel::Xai(id),
                        _ => unreachable!("static provider list"),
                    });
                }
            }
        }
        for model in fallback
            .iter()
            .filter(|model| model.provider() == provider)
            .cloned()
        {
            let id = match &model {
                HostedModel::Gemini(id) | HostedModel::Cerebras(id) | HostedModel::Xai(id) => id,
            };
            if known.insert((provider, id.clone())) {
                models.push(model);
            }
        }
    }
}

fn rank_catalog_models(ids: Vec<String>, tier: Tier) -> Vec<String> {
    let mut ids = ids
        .into_iter()
        .filter(|id| compatible_model_id(id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    ids.sort_by_key(|id| (model_rank(id, tier), id.to_ascii_lowercase()));
    ids.truncate(MAX_CATALOG_MODELS);
    ids
}

fn model_rank(id: &str, tier: Tier) -> u8 {
    let id = id.to_ascii_lowercase();
    let flash = id.contains("flash") || id.contains("mini") || id.contains("small");
    let reasoning = id.contains("reason") || id.contains("thinking") || id.contains("oss-120b");
    match tier {
        Tier::Fast if flash => 0,
        Tier::Fast => 1,
        Tier::Balanced if flash => 1,
        Tier::Balanced => 0,
        Tier::Deep if reasoning => 0,
        Tier::Deep => 1,
    }
}

fn compatible_model_id(id: &str) -> bool {
    let lowered = id.to_ascii_lowercase();
    valid_model_id(id)
        && [
            "gemini", "gemma", "gpt", "qwen", "llama", "mistral", "deepseek", "grok",
        ]
        .iter()
        .any(|family| lowered.contains(family))
        && !["embedding", "image", "audio", "tts", "vision"]
            .iter()
            .any(|unsupported| lowered.contains(unsupported))
}

pub(super) fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn provider_prompt(evidence: &str) -> Result<String> {
    let prompt = format!("Evidence JSON:\n{evidence}");
    if prompt.len() > MAX_EVIDENCE_BYTES + 4_000 {
        bail!("mention prompt is too large");
    }
    Ok(prompt)
}

fn gemini_catalog(key: &str) -> Result<Vec<String>> {
    let output = provider_get(
        "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000",
        "x-goog-api-key",
        key,
    )?;
    Ok(serde_json::from_str::<Value>(&output)
        .ok()
        .and_then(|value| value["models"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|model| {
            model["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|method| method.as_str() == Some("generateContent"))
                })
        })
        .filter_map(|model| {
            model["name"]
                .as_str()?
                .strip_prefix("models/")
                .map(str::to_owned)
        })
        .collect())
}

fn cerebras_catalog(key: &str) -> Result<Vec<String>> {
    let output = provider_get(
        "https://api.cerebras.ai/v1/models",
        "Authorization",
        &format!("Bearer {key}"),
    )?;
    Ok(serde_json::from_str::<Value>(&output)
        .ok()
        .and_then(|value| value["data"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .collect())
}

fn xai_catalog(key: &str) -> Result<Vec<String>> {
    let output = provider_get(
        "https://api.x.ai/v1/models",
        "Authorization",
        &format!("Bearer {key}"),
    )?;
    Ok(serde_json::from_str::<Value>(&output)
        .ok()
        .and_then(|value| value["data"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .collect())
}

fn gemini_answer(
    prompt: &str,
    model: &str,
    key: &str,
    instructions: &str,
    schema: &Value,
) -> Result<Value> {
    let body = gemini_request(prompt, instructions, schema);
    let output = provider_request(
        &format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"),
        "x-goog-api-key",
        key,
        &body,
    )?;
    let text = serde_json::from_str::<Value>(&output)
        .ok()
        .and_then(|value| {
            value["candidates"]
                .as_array()?
                .first()?
                .get("content")?
                .get("parts")?
                .as_array()?
                .first()?
                .get("text")?
                .as_str()
                .map(str::to_owned)
        })
        .ok_or_else(|| anyhow!("Gemini returned an invalid response"))?;
    strict_json_response(&text)
}

fn gemini_request(prompt: &str, instructions: &str, schema: &Value) -> Value {
    json!({
        "systemInstruction": {"parts": [{"text": instructions}]},
        "contents": [{"role": "user", "parts": [{"text": prompt}]}],
        "generationConfig": {
            "maxOutputTokens": 1_600,
            "responseMimeType": "application/json",
            "responseJsonSchema": schema
        },
    })
}

fn cerebras_answer(
    prompt: &str,
    model: &str,
    key: &str,
    instructions: &str,
    schema: &Value,
) -> Result<Value> {
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": instructions},
            {"role": "user", "content": prompt},
        ],
        "max_completion_tokens": 1_600,
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "rady_answer",
                "strict": true,
                "schema": schema
            }
        },
    });
    let output = provider_request(
        "https://api.cerebras.ai/v1/chat/completions",
        "Authorization",
        &format!("Bearer {key}"),
        &body,
    )?;
    let text = serde_json::from_str::<Value>(&output)
        .ok()
        .and_then(|value| {
            value["choices"]
                .as_array()?
                .first()?
                .get("message")?
                .get("content")?
                .as_str()
                .map(str::to_owned)
        })
        .ok_or_else(|| anyhow!("Cerebras returned an invalid response"))?;
    strict_json_response(&text)
}

fn xai_answer(
    prompt: &str,
    model: &str,
    key: &str,
    instructions: &str,
    schema: &Value,
) -> Result<Value> {
    let body = json!({
        "model": model,
        "input": [
            {"role": "system", "content": instructions},
            {"role": "user", "content": prompt},
        ],
        "max_output_tokens": 1_600,
        "reasoning": {"effort": "low"},
        "store": false,
        "text": {
            "format": {
                "type": "json_schema",
                "name": "rady_answer",
                "strict": true,
                "schema": schema
            }
        },
    });
    let output = provider_request(
        "https://api.x.ai/v1/responses",
        "Authorization",
        &format!("Bearer {key}"),
        &body,
    )?;
    let text =
        xai_response_text(&output).ok_or_else(|| anyhow!("xAI returned an invalid response"))?;
    strict_json_response(&text)
}

fn xai_response_text(output: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(output).ok()?;
    value["output"]
        .as_array()?
        .iter()
        .find(|item| item["type"] == "message")?
        .get("content")?
        .as_array()?
        .iter()
        .find(|content| content["type"] == "output_text")?
        .get("text")?
        .as_str()
        .map(str::to_owned)
}

fn provider_request(url: &str, header_name: &str, secret: &str, body: &Value) -> Result<String> {
    provider_call(
        url,
        header_name,
        secret,
        "POST",
        Some(serde_json::to_vec(body)?),
    )
}

fn provider_get(url: &str, header_name: &str, secret: &str) -> Result<String> {
    provider_call(url, header_name, secret, "GET", None)
}

fn provider_call(
    url: &str,
    header_name: &str,
    secret: &str,
    method: &str,
    body: Option<Vec<u8>>,
) -> Result<String> {
    let curl = agent::which("curl").ok_or_else(|| anyhow!("install curl to answer mentions"))?;
    let directory = tempdir()?;
    let config = directory.path().join("curl.conf");
    std::fs::write(
        &config,
        format!("header = \"{}: {}\"\n", header_name, curl_escape(secret)),
    )?;
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
    )?;
    provider_response(output.code, &output.stdout)
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
        MAX_PROVIDER_RESPONSE_BYTES.to_string(),
        "--write-out".to_owned(),
        format!("{HTTP_STATUS_MARKER}%{{http_code}}"),
        url.to_owned(),
    ]);
    arguments
}

fn provider_response(code: i32, output: &str) -> Result<String> {
    let (body, status) = output
        .rsplit_once(HTTP_STATUS_MARKER)
        .ok_or_else(|| anyhow!("request returned no HTTP status"))?;
    let status = status
        .parse::<u16>()
        .map_err(|_| anyhow!("request returned an invalid HTTP status"))?;
    if code != 0 {
        match code {
            6 => bail!("request could not resolve the provider"),
            7 => bail!("request could not connect to the provider"),
            22 if status == 402 => bail!("provider reported payment required (HTTP 402)"),
            22 if status != 0 => bail!("request was rejected (HTTP {status})"),
            28 => bail!("request timed out"),
            63 => bail!("response exceeded {MAX_PROVIDER_RESPONSE_BYTES} bytes"),
            _ => bail!("request failed (transport {code})"),
        }
    }
    if !(200..300).contains(&status) {
        bail!("request returned HTTP {status}");
    }
    if body.len() > MAX_PROVIDER_RESPONSE_BYTES {
        bail!("response exceeded {MAX_PROVIDER_RESPONSE_BYTES} bytes");
    }
    Ok(body.to_owned())
}

fn curl_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn valid_api_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1_024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"' && byte != b'\\')
}

#[cfg(test)]
fn answer_from_json(value: &str) -> Result<String> {
    answer_from_value(&strict_json_response(value)?)
}

fn strict_json_response(value: &str) -> Result<Value> {
    if value.len() > MAX_PROVIDER_RESPONSE_BYTES {
        bail!("hosted model returned an oversized response");
    }
    serde_json::from_str(value)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| anyhow!("hosted model returned invalid JSON"))
}

pub(super) fn answer_from_value(value: &Value) -> Result<String> {
    let object = value
        .as_object()
        .filter(|object| {
            object
                .keys()
                .all(|key| matches!(key.as_str(), "answer" | "follow_ups"))
        })
        .ok_or_else(|| anyhow!("hosted model returned an invalid response"))?;
    let mut answer = object
        .get("answer")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .map(|answer| answer.trim().to_owned())
        .filter(|answer| !answer.is_empty() && answer.chars().count() <= MAX_ANSWER)
        .ok_or_else(|| anyhow!("hosted model returned an invalid response"))?;
    let follow_ups = object
        .get("follow_ups")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| anyhow!("hosted model returned invalid follow-up questions"))
        })
        .transpose()?
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|question| !question.is_empty() && question.chars().count() <= 240)
        .take(3)
        .collect::<Vec<_>>();
    if !follow_ups.is_empty() {
        answer.push_str("\n\nYou could ask next:\n");
        for question in follow_ups {
            answer.push_str("\n- ");
            answer.push_str(question);
        }
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_hosted_models_and_validates_strict_answers() {
        for (tier, gemini_allowed, cerebras_allowed, xai_allowed, expected) in [
            (
                Tier::Fast,
                true,
                true,
                true,
                vec![
                    HostedModel::Gemini("gemini-3.5-flash-lite".to_owned()),
                    HostedModel::Cerebras("gpt-oss-120b".to_owned()),
                    HostedModel::Xai("grok-4.7".to_owned()),
                ],
            ),
            (
                Tier::Balanced,
                true,
                true,
                false,
                vec![
                    HostedModel::Gemini("gemini-3.8-flash".to_owned()),
                    HostedModel::Cerebras("qwen-3.8-27b".to_owned()),
                    HostedModel::Cerebras("gpt-oss-120b".to_owned()),
                ],
            ),
            (
                Tier::Deep,
                false,
                true,
                true,
                vec![
                    HostedModel::Cerebras("qwen-3.8-27b".to_owned()),
                    HostedModel::Cerebras("gpt-oss-120b".to_owned()),
                    HostedModel::Xai("grok-4.7".to_owned()),
                ],
            ),
        ] {
            assert_eq!(
                hosted_models(tier, gemini_allowed, cerebras_allowed, xai_allowed),
                expected
            );
        }
        for (private, opted_in, allowed) in [
            (Some(false), false, true),
            (Some(true), false, false),
            (None, false, false),
            (Some(true), true, true),
        ] {
            assert_eq!(external_provider_permitted(private, opted_in), allowed);
        }
        assert_eq!(answer_from_json(r#"{"answer":"ready"}"#).unwrap(), "ready");
        let schema = answer_schema();
        let gemini = gemini_request("evidence", "instructions", &schema);
        assert_eq!(
            gemini["generationConfig"]["responseJsonSchema"]["type"],
            "object"
        );
        assert_eq!(
            xai_response_text(
                r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"answer\":\"ready\"}"}]}]}"#
            )
            .as_deref(),
            Some(r#"{"answer":"ready"}"#)
        );
        for (code, output, expected) in [
            (0, "{\"answer\":\"ready\"}\nRADY_HTTP_STATUS:200", None),
            (
                22,
                "provider body must stay hidden\nRADY_HTTP_STATUS:402",
                Some("provider reported payment required (HTTP 402)"),
            ),
            (28, "\nRADY_HTTP_STATUS:000", Some("request timed out")),
        ] {
            let result = provider_response(code, output);
            match expected {
                Some(message) => assert_eq!(result.unwrap_err().to_string(), message),
                None => assert_eq!(result.unwrap(), r#"{"answer":"ready"}"#),
            }
        }
    }

    #[test]
    fn catalog_ranking_rejects_non_generation_models_and_stays_bounded() {
        let ids = ["gemini-pro", "gemini-flash", "embedding-001"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            rank_catalog_models(ids, Tier::Fast),
            vec!["gemini-flash".to_owned(), "gemini-pro".to_owned()]
        );
        assert!(!valid_model_id("gemini/unsafe"));
        for (method, body, expected_body) in [("GET", false, false), ("POST", true, true)] {
            let arguments = provider_arguments(
                method,
                Path::new("/tmp/rady-curl.conf"),
                "https://provider.example/v1/models",
                body,
            );
            assert!(
                arguments
                    .windows(2)
                    .any(|pair| pair == ["--proto", "=https"])
            );
            assert!(arguments.iter().any(|argument| argument == "--tlsv1.2"));
            assert!(arguments.iter().any(|argument| argument == "--no-location"));
            assert_eq!(
                arguments.iter().any(|argument| argument == "--data-binary"),
                expected_body
            );
        }
    }
}
