use serde_json::{Value, json};

use super::{Credentials, FailureKind, ProviderFailure, http, strict_json_response};

pub(super) fn catalog(credentials: &Credentials) -> Result<Vec<String>, ProviderFailure> {
    let output = http::get(
        "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000",
        "x-goog-api-key",
        &credentials.key,
    )?;
    let value = serde_json::from_str::<Value>(&output).map_err(|_| {
        ProviderFailure::new(
            FailureKind::InvalidResponse,
            "returned an invalid model catalog",
        )
    })?;
    Ok(value["models"]
        .as_array()
        .into_iter()
        .flatten()
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

pub(super) fn answer(
    prompt: &str,
    model: &str,
    credentials: &Credentials,
    instructions: &str,
    schema: &Value,
) -> Result<Value, ProviderFailure> {
    let body = json!({
        "systemInstruction": {"parts": [{"text": instructions}]},
        "contents": [{"role": "user", "parts": [{"text": prompt}]}],
        "generationConfig": {
            "maxOutputTokens": 1_600,
            "responseMimeType": "application/json",
            "responseJsonSchema": schema
        }
    });
    let body = serde_json::to_vec(&body).map_err(|_| {
        ProviderFailure::new(FailureKind::Model, "could not encode the provider request")
    })?;
    let output = http::post(
        &format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"),
        "x-goog-api-key",
        &credentials.key,
        body,
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
        .ok_or_else(|| {
            ProviderFailure::new(FailureKind::InvalidResponse, "returned an invalid response")
        })?;
    strict_json_response(&text)
}
