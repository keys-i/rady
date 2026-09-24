use serde_json::{Value, json};

use super::{Credentials, FailureKind, Provider, ProviderFailure, http, strict_json_response};

pub(super) fn catalog(
    provider: Provider,
    credentials: &Credentials,
) -> Result<Vec<String>, ProviderFailure> {
    let url = match provider {
        Provider::Groq => "https://api.groq.com/openai/v1/models",
        Provider::Cerebras => "https://api.cerebras.ai/v1/models",
        _ => {
            return Err(ProviderFailure::new(
                FailureKind::Model,
                "provider has no compatible model catalog",
            ));
        }
    };
    let output = http::get(url, "Authorization", &format!("Bearer {}", credentials.key))?;
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
    provider: Provider,
    prompt: &str,
    model: &str,
    credentials: &Credentials,
    instructions: &str,
    schema: &Value,
) -> Result<Value, ProviderFailure> {
    let url = match provider {
        Provider::Groq => "https://api.groq.com/openai/v1/chat/completions".to_owned(),
        Provider::Cloudflare => format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/ai/v1/chat/completions",
            credentials.account_id.as_deref().unwrap_or_default()
        ),
        Provider::Cerebras => "https://api.cerebras.ai/v1/chat/completions".to_owned(),
        Provider::OpenRouter => "https://openrouter.ai/api/v1/chat/completions".to_owned(),
        _ => {
            return Err(ProviderFailure::new(
                FailureKind::Model,
                "provider is not OpenAI compatible",
            ));
        }
    };
    let body = request_body(provider, model, prompt, instructions, schema);
    let body = serde_json::to_vec(&body).map_err(|_| {
        ProviderFailure::new(FailureKind::Model, "could not encode the provider request")
    })?;
    let output = http::post(
        &url,
        "Authorization",
        &format!("Bearer {}", credentials.key),
        body,
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
        .ok_or_else(|| {
            ProviderFailure::new(FailureKind::InvalidResponse, "returned an invalid response")
        })?;
    strict_json_response(&text)
}

fn request_body(
    provider: Provider,
    model: &str,
    prompt: &str,
    instructions: &str,
    schema: &Value,
) -> Value {
    let instructions = if matches!(provider, Provider::Cloudflare | Provider::OpenRouter) {
        format!(
            "{instructions}\n\nReturn only a JSON value matching this response schema:\n{schema}"
        )
    } else {
        instructions.to_owned()
    };
    let mut body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": instructions},
            {"role": "user", "content": prompt}
        ]
    });
    let token_field = if provider == Provider::Cerebras {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    body[token_field] = json!(1_600);
    match provider {
        Provider::Groq | Provider::Cerebras => {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "rady_answer",
                    "strict": true,
                    "schema": schema
                }
            });
        }
        Provider::Cloudflare | Provider::OpenRouter => {}
        _ => unreachable!("OpenAI-compatible providers were matched above"),
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shape_matches_each_provider_capability() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"]
        });
        for (provider, token_field, structured) in [
            (Provider::Groq, "max_tokens", true),
            (Provider::Cerebras, "max_completion_tokens", true),
            (Provider::Cloudflare, "max_tokens", false),
            (Provider::OpenRouter, "max_tokens", false),
        ] {
            let body = request_body(provider, "model", "prompt", "instructions", &schema);
            assert_eq!(body[token_field], 1_600);
            assert_eq!(body.get("response_format").is_some(), structured);
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert_eq!(system.contains("response schema"), !structured);
            if !structured {
                assert!(system.contains("\"answer\""));
            }
        }
    }
}
