use serde_json::{Value, json};

use super::{Credentials, FailureKind, ProviderFailure, http, strict_json_response};

pub(super) fn catalog(credentials: &Credentials) -> Result<Vec<String>, ProviderFailure> {
    let output = http::get(
        "https://api.x.ai/v1/models",
        "Authorization",
        &format!("Bearer {}", credentials.key),
    )?;
    let value = serde_json::from_str::<Value>(&output).map_err(|_| {
        ProviderFailure::new(
            FailureKind::InvalidResponse,
            "returned an invalid model catalog",
        )
    })?;
    Ok(value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
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
        "model": model,
        "input": [
            {"role": "system", "content": instructions},
            {"role": "user", "content": prompt}
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
        }
    });
    let body = serde_json::to_vec(&body).map_err(|_| {
        ProviderFailure::new(FailureKind::Model, "could not encode the provider request")
    })?;
    let output = http::post(
        "https://api.x.ai/v1/responses",
        "Authorization",
        &format!("Bearer {}", credentials.key),
        body,
    )?;
    let text = response_text(&output).ok_or_else(|| {
        ProviderFailure::new(FailureKind::InvalidResponse, "returned an invalid response")
    })?;
    strict_json_response(&text)
}

fn response_text(output: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_output_text() {
        assert_eq!(
            response_text(
                r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"answer\":\"ready\"}"}]}]}"#
            )
            .as_deref(),
            Some(r#"{"answer":"ready"}"#)
        );
    }
}
