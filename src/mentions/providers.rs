use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

use super::{MAX_ANSWER, MAX_EVIDENCE_BYTES};
use crate::Result;
use crate::routing::Tier;

mod gemini;
mod http;
mod openai;
mod xai;

const MAX_CATALOG_MODELS: usize = 16;
const MAX_MODELS_PER_PROVIDER: usize = 2;
const MAX_FAILURES: usize = 8;
const MAX_REJECTED_MODELS: usize = 32;
const CATALOG_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Provider {
    Gemini,
    Groq,
    Cloudflare,
    Cerebras,
    OpenRouter,
    Xai,
}

impl Provider {
    const ALL: [Self; 6] = [
        Self::Gemini,
        Self::Groq,
        Self::Cloudflare,
        Self::Cerebras,
        Self::OpenRouter,
        Self::Xai,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Gemini => "Gemini",
            Self::Groq => "Groq",
            Self::Cloudflare => "Cloudflare Workers AI",
            Self::Cerebras => "Cerebras",
            Self::OpenRouter => "OpenRouter",
            Self::Xai => "xAI",
        }
    }

    const fn key_name(self) -> &'static str {
        match self {
            Self::Gemini => "RADY_GEMINI_API_KEY",
            Self::Groq => "RADY_GROQ_API_KEY",
            Self::Cloudflare => "RADY_CLOUDFLARE_API_TOKEN",
            Self::Cerebras => "RADY_CEREBRAS_API_KEY",
            Self::OpenRouter => "RADY_OPENROUTER_API_KEY",
            Self::Xai => "RADY_XAI_API_KEY",
        }
    }

    const fn private_opt_in(self) -> &'static str {
        match self {
            Self::Gemini => "RADY_GEMINI_PRIVATE_OK",
            Self::Groq => "RADY_GROQ_PRIVATE_OK",
            Self::Cloudflare => "RADY_CLOUDFLARE_PRIVATE_OK",
            Self::Cerebras => "RADY_CEREBRAS_PRIVATE_OK",
            Self::OpenRouter => "RADY_OPENROUTER_PRIVATE_OK",
            Self::Xai => "RADY_XAI_PRIVATE_OK",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HostedModel {
    provider: Provider,
    id: String,
}

impl HostedModel {
    fn label(&self) -> String {
        format!("{} {}", self.provider.name(), self.id)
    }
}

struct Credentials {
    key: String,
    account_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureKind {
    Authentication,
    Payment,
    RateLimit,
    Model,
    Transient,
    InvalidResponse,
}

#[derive(Clone, Debug)]
struct ProviderFailure {
    kind: FailureKind,
    detail: String,
    retry_after: Option<Duration>,
}

impl ProviderFailure {
    fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            retry_after: None,
        }
    }

    fn retry_after(mut self, retry_after: Option<Duration>) -> Self {
        self.retry_after = retry_after;
        self
    }
}

impl fmt::Display for ProviderFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for ProviderFailure {}

#[derive(Debug)]
struct HostedUnavailable(String);

impl fmt::Display for HostedUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for HostedUnavailable {}

#[derive(Default)]
struct ProviderState {
    catalog: Vec<String>,
    catalog_until: Option<Instant>,
    cooldown_until: Option<Instant>,
    rejected_models: BTreeSet<String>,
    disabled: bool,
}

#[derive(Default)]
struct RouterState {
    providers: BTreeMap<Provider, ProviderState>,
    rotation: usize,
}

// ponytail: one process-wide router is enough while provider calls stay sequential
static ROUTER: OnceLock<Mutex<RouterState>> = OnceLock::new();

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
    let prompt = provider_prompt(evidence)?;
    chained_request(&prompt, instructions, schema, tier, repository_private)
}

pub(crate) fn is_hosted_unavailable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<HostedUnavailable>().is_some()
}

fn chained_request(
    prompt: &str,
    instructions: &str,
    schema: &Value,
    tier: Tier,
    repository_private: Option<bool>,
) -> Result<Value> {
    let mut prompt = prompt.to_owned();
    let mut scout_provider = None;
    if tier == Tier::Deep {
        let brief_schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"brief": {"type": "string", "minLength": 1, "maxLength": 4000}},
            "required": ["brief"]
        });
        if let Ok((brief, used)) = request_models(
            &prompt,
            "Produce a concise evidence brief of concrete facts, uncertainties and decisions for another model. Do not provide private chain-of-thought. Return only JSON matching the schema.",
            &brief_schema,
            Tier::Fast,
            repository_private,
            None,
            Some(1),
        ) {
            if let Some(brief) = brief["brief"].as_str() {
                prompt.push_str("\n\nIndependent evidence brief:\n");
                prompt.extend(brief.chars().take(4_000));
                scout_provider = Some(used);
            }
        }
    }
    request_models(
        &prompt,
        instructions,
        schema,
        tier,
        repository_private,
        scout_provider,
        None,
    )
    .map(|(answer, _)| answer)
}

#[allow(clippy::too_many_arguments)]
fn request_models(
    prompt: &str,
    instructions: &str,
    schema: &Value,
    tier: Tier,
    repository_private: Option<bool>,
    prefer_last: Option<Provider>,
    provider_limit: Option<usize>,
) -> Result<(Value, Provider)> {
    let permitted = Provider::ALL
        .into_iter()
        .filter(|provider| provider_permitted(*provider, repository_private))
        .collect::<Vec<_>>();
    let any_key = permitted.iter().any(|provider| {
        env::var(provider.key_name())
            .ok()
            .is_some_and(|key| !key.is_empty())
    });
    let mut providers = provider_order(tier, &permitted);
    if let Some(prefer_last) = prefer_last {
        providers.sort_by_key(|provider| *provider == prefer_last);
    }
    let mut failures = Vec::new();
    let mut attempted = BTreeSet::new();
    let mut attempted_providers = 0;
    let mut had_credentials = false;
    for provider in providers {
        let credentials = match credentials(provider) {
            Ok(Some(credentials)) => {
                had_credentials = true;
                credentials
            }
            Ok(None) => continue,
            Err(failure) => {
                record_failure(provider, &failure);
                push_failure(&mut failures, provider.name(), &failure);
                continue;
            }
        };
        if provider_limit.is_some_and(|limit| attempted_providers >= limit) {
            break;
        }
        attempted_providers += 1;
        let mut models = known_provider_models(provider, tier);
        let mut catalog_checked = false;
        let mut next_model = 0;
        loop {
            let mut provider_blocked = false;
            while let Some(model) = models.get(next_model).cloned() {
                next_model += 1;
                if !attempted.insert((provider, model.id.clone())) {
                    continue;
                }
                match call_with_retry(&model, &credentials, prompt, instructions, schema) {
                    Ok(answer) => {
                        record_success(provider);
                        return Ok((answer, provider));
                    }
                    Err(failure) => {
                        provider_blocked = matches!(
                            failure.kind,
                            FailureKind::Authentication
                                | FailureKind::Payment
                                | FailureKind::RateLimit
                                | FailureKind::Transient
                        );
                        record_failure(provider, &failure);
                        if failure.kind == FailureKind::Model {
                            record_rejected_model(provider, &model.id);
                        }
                        push_failure(&mut failures, &model.label(), &failure);
                        if provider_blocked {
                            break;
                        }
                    }
                }
            }
            if provider_blocked || catalog_checked {
                break;
            }
            catalog_checked = true;
            match catalog_provider_models(provider, tier, &credentials) {
                Ok(discovered) => {
                    let before = models.len();
                    models.extend(
                        discovered.into_iter().filter(|model| {
                            !attempted.contains(&(model.provider, model.id.clone()))
                        }),
                    );
                    if models.len() == before {
                        break;
                    }
                }
                Err(failure) => {
                    record_failure(provider, &failure);
                    push_failure(&mut failures, provider.name(), &failure);
                    break;
                }
            }
        }
    }

    if !failures.is_empty() {
        return Err(anyhow!(HostedUnavailable(format!(
            "hosted models are temporarily unavailable: {}; nothing was posted",
            failures.join("; ")
        ))));
    }
    if any_key || had_credentials {
        return Err(anyhow!(HostedUnavailable(
            "hosted models are cooling down; nothing was posted and the next service pass will retry"
                .to_owned()
        )));
    }
    bail!("no permitted hosted model key is available and Codex is not signed in")
}

fn call_with_retry(
    model: &HostedModel,
    credentials: &Credentials,
    prompt: &str,
    instructions: &str,
    schema: &Value,
) -> std::result::Result<Value, ProviderFailure> {
    let call = || match model.provider {
        Provider::Gemini => gemini::answer(prompt, &model.id, credentials, instructions, schema),
        Provider::Groq | Provider::Cloudflare | Provider::Cerebras | Provider::OpenRouter => {
            openai::answer(
                model.provider,
                prompt,
                &model.id,
                credentials,
                instructions,
                schema,
            )
        }
        Provider::Xai => xai::answer(prompt, &model.id, credentials, instructions, schema),
    };
    match call() {
        Err(failure) if failure.kind == FailureKind::Transient => {
            thread::sleep(transient_retry_delay());
            call()
        }
        result => result,
    }
}

fn transient_retry_delay() -> Duration {
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_millis() as u64 % 251);
    Duration::from_millis(250 + jitter)
}

fn credentials(provider: Provider) -> std::result::Result<Option<Credentials>, ProviderFailure> {
    let Some(key) = env::var(provider.key_name())
        .ok()
        .filter(|key| !key.is_empty())
    else {
        return Ok(None);
    };
    if !valid_api_key(&key) {
        return Err(ProviderFailure::new(
            FailureKind::Authentication,
            format!("{} is invalid", provider.key_name()),
        ));
    }
    let account_id = if provider == Provider::Cloudflare {
        let value = env::var("RADY_CLOUDFLARE_ACCOUNT_ID").unwrap_or_default();
        if !valid_account_id(&value) {
            return Err(ProviderFailure::new(
                FailureKind::Authentication,
                "RADY_CLOUDFLARE_ACCOUNT_ID is missing or invalid",
            ));
        }
        Some(value)
    } else {
        None
    };
    Ok(Some(Credentials { key, account_id }))
}

fn known_provider_models(provider: Provider, tier: Tier) -> Vec<HostedModel> {
    models_from_ids(
        provider,
        default_models(provider, tier)
            .iter()
            .map(|id| (*id).to_owned()),
    )
}

fn catalog_provider_models(
    provider: Provider,
    tier: Tier,
    credentials: &Credentials,
) -> std::result::Result<Vec<HostedModel>, ProviderFailure> {
    let discovered = match cached_catalog(provider) {
        Some(models) => models,
        None => {
            let models = discover_models(provider, credentials)?;
            if !models.is_empty() {
                store_catalog(provider, &models);
            }
            models
        }
    };
    Ok(models_from_ids(
        provider,
        rank_catalog_models(discovered, tier),
    ))
}

fn models_from_ids(provider: Provider, ids: impl IntoIterator<Item = String>) -> Vec<HostedModel> {
    let rejected = with_router(|router| {
        router
            .providers
            .entry(provider)
            .or_default()
            .rejected_models
            .clone()
    });
    let mut seen = BTreeSet::new();
    ids.into_iter()
        .filter(|id| compatible_model_id(id) && !rejected.contains(id) && seen.insert(id.clone()))
        .take(MAX_MODELS_PER_PROVIDER)
        .map(|id| HostedModel { provider, id })
        .collect()
}

fn discover_models(
    provider: Provider,
    credentials: &Credentials,
) -> std::result::Result<Vec<String>, ProviderFailure> {
    match provider {
        Provider::Gemini => gemini::catalog(credentials),
        Provider::Groq | Provider::Cerebras => openai::catalog(provider, credentials),
        Provider::Cloudflare | Provider::OpenRouter => Ok(Vec::new()),
        Provider::Xai => xai::catalog(credentials),
    }
}

fn default_models(provider: Provider, tier: Tier) -> &'static [&'static str] {
    match (provider, tier) {
        (Provider::Gemini, Tier::Fast) => &["gemini-3.5-flash-lite", "gemini-3.1-flash-lite"],
        (Provider::Gemini, Tier::Balanced | Tier::Deep) => {
            &["gemini-3.8-flash", "gemini-3.5-flash"]
        }
        (Provider::Groq, Tier::Fast) => &["qwen/qwen3.8-27b", "openai/gpt-oss-20b"],
        (Provider::Groq, Tier::Balanced | Tier::Deep) => {
            &["openai/gpt-oss-120b", "qwen/qwen3.8-27b"]
        }
        (Provider::Cloudflare, Tier::Fast) => &["@cf/openai/gpt-oss-20b"],
        (Provider::Cloudflare, Tier::Balanced | Tier::Deep) => {
            &["@cf/openai/gpt-oss-120b", "@cf/openai/gpt-oss-20b"]
        }
        (Provider::Cerebras, _) => &["gpt-oss-120b", "llama3.1-8b"],
        (Provider::OpenRouter, _) => &["openrouter/free"],
        (Provider::Xai, _) => &["grok-4.7"],
    }
}

fn provider_order(tier: Tier, permitted: &[Provider]) -> Vec<Provider> {
    let now = Instant::now();
    let mut providers = with_router(|router| {
        permitted
            .iter()
            .copied()
            .filter(|provider| {
                let state = router.providers.entry(*provider).or_default();
                !state.disabled && state.cooldown_until.is_none_or(|until| until <= now)
            })
            .collect::<Vec<_>>()
    });
    let primary_count = providers
        .iter()
        .position(|provider| matches!(provider, Provider::OpenRouter | Provider::Xai))
        .unwrap_or(providers.len());
    if tier != Tier::Deep && primary_count > 1 {
        let offset = with_router(|router| {
            let offset = router.rotation % primary_count;
            router.rotation = router.rotation.wrapping_add(1);
            offset
        });
        providers[..primary_count].rotate_left(offset);
    }
    providers
}

fn cached_catalog(provider: Provider) -> Option<Vec<String>> {
    let now = Instant::now();
    with_router(|router| {
        let state = router.providers.entry(provider).or_default();
        state
            .catalog_until
            .filter(|until| *until > now)
            .map(|_| state.catalog.clone())
    })
}

fn store_catalog(provider: Provider, models: &[String]) {
    with_router(|router| {
        let state = router.providers.entry(provider).or_default();
        state.catalog = models.iter().take(MAX_CATALOG_MODELS).cloned().collect();
        state.catalog_until = Some(Instant::now() + CATALOG_TTL);
    });
}

fn record_success(provider: Provider) {
    with_router(|router| {
        let state = router.providers.entry(provider).or_default();
        state.cooldown_until = None;
    });
}

fn record_failure(provider: Provider, failure: &ProviderFailure) {
    with_router(|router| {
        let state = router.providers.entry(provider).or_default();
        match failure.kind {
            FailureKind::Authentication => state.disabled = true,
            FailureKind::Payment => {
                state.cooldown_until = Some(Instant::now() + Duration::from_secs(24 * 60 * 60));
            }
            FailureKind::RateLimit => {
                let delay = failure
                    .retry_after
                    .unwrap_or(Duration::from_secs(60))
                    .clamp(Duration::from_secs(1), Duration::from_secs(6 * 60 * 60));
                state.cooldown_until = Some(Instant::now() + delay);
            }
            FailureKind::Transient => {
                state.cooldown_until = Some(Instant::now() + Duration::from_secs(30));
            }
            FailureKind::Model | FailureKind::InvalidResponse => {}
        }
    });
}

fn record_rejected_model(provider: Provider, model: &str) {
    with_router(|router| {
        let rejected = &mut router
            .providers
            .entry(provider)
            .or_default()
            .rejected_models;
        if rejected.len() >= MAX_REJECTED_MODELS && !rejected.contains(model) {
            rejected.pop_first();
        }
        rejected.insert(model.to_owned());
    });
}

fn with_router<T>(operation: impl FnOnce(&mut RouterState) -> T) -> T {
    let mut router = ROUTER
        .get_or_init(|| Mutex::new(RouterState::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    operation(&mut router)
}

fn push_failure(failures: &mut Vec<String>, label: &str, failure: &ProviderFailure) {
    if failures.len() < MAX_FAILURES {
        failures.push(format!("{label}: {failure}"));
    }
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
        "required": ["answer", "follow_ups"]
    })
}

pub(super) fn bool_environment(name: &str) -> Option<bool> {
    match env::var(name).ok()?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn provider_permitted(provider: Provider, repository_private: Option<bool>) -> bool {
    repository_private == Some(false) || bool_environment(provider.private_opt_in()) == Some(true)
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
    let small = id.contains("flash")
        || id.contains("lite")
        || id.contains("mini")
        || id.contains("20b")
        || id.contains("8b");
    let reasoning = id.contains("reason")
        || id.contains("thinking")
        || id.contains("120b")
        || id.contains("qwen3");
    match tier {
        Tier::Fast if small => 0,
        Tier::Fast => 1,
        Tier::Balanced if reasoning => 0,
        Tier::Balanced => 1,
        Tier::Deep if reasoning => 0,
        Tier::Deep => 1,
    }
}

fn compatible_model_id(id: &str) -> bool {
    let lowered = id.to_ascii_lowercase();
    valid_model_identifier(id)
        && [
            "gemini", "gemma", "gpt", "qwen", "llama", "mistral", "deepseek", "grok",
        ]
        .iter()
        .any(|family| lowered.contains(family))
        && !["embedding", "image", "audio", "tts", "vision"]
            .iter()
            .any(|unsupported| lowered.contains(unsupported))
}

fn valid_model_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 160
        && !id.contains("://")
        && !id.contains("..")
        && !id.starts_with('/')
        && id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':' | b'@')
        })
}

pub(super) fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_api_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1_024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"' && byte != b'\\')
}

fn valid_account_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn provider_prompt(evidence: &str) -> Result<String> {
    let prompt = format!("Evidence JSON:\n{evidence}");
    if prompt.len() > MAX_EVIDENCE_BYTES + 4_000 {
        bail!("mention prompt is too large");
    }
    Ok(prompt)
}

fn strict_json_response(value: &str) -> std::result::Result<Value, ProviderFailure> {
    if value.len() > http::MAX_RESPONSE_BYTES {
        return Err(ProviderFailure::new(
            FailureKind::InvalidResponse,
            "returned an oversized response",
        ));
    }
    serde_json::from_str(value)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| ProviderFailure::new(FailureKind::InvalidResponse, "returned invalid JSON"))
}

#[cfg(test)]
fn answer_from_json(value: &str) -> Result<String> {
    answer_from_value(&strict_json_response(value).map_err(anyhow::Error::new)?)
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
    fn routes_supported_providers_and_validates_identifiers() {
        let cases = [
            (
                Provider::Groq,
                Tier::Deep,
                &["openai/gpt-oss-120b", "qwen/qwen3.8-27b"][..],
            ),
            (
                Provider::Cloudflare,
                Tier::Fast,
                &["@cf/openai/gpt-oss-20b"][..],
            ),
            (
                Provider::OpenRouter,
                Tier::Balanced,
                &["openrouter/free"][..],
            ),
            (
                Provider::Cerebras,
                Tier::Deep,
                &["gpt-oss-120b", "llama3.1-8b"][..],
            ),
        ];
        for (provider, tier, expected) in cases {
            assert_eq!(default_models(provider, tier), expected);
        }
        assert!(valid_model_identifier("@cf/openai/gpt-oss-120b"));
        assert!(valid_model_identifier("meta-llama/llama-4:free"));
        assert!(!valid_model_identifier("https://provider.invalid/model"));
        assert!(valid_slug("radduck"));
        assert!(!valid_slug("radduck/model"));
    }

    #[test]
    fn ranks_models_and_keeps_answers_strict() {
        let ids = ["gemini-pro", "gemini-flash", "embedding-001"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            rank_catalog_models(ids, Tier::Fast),
            vec!["gemini-flash".to_owned(), "gemini-pro".to_owned()]
        );
        assert_eq!(answer_from_json(r#"{"answer":"ready"}"#).unwrap(), "ready");
        let error = anyhow!(HostedUnavailable("cooling down".to_owned()));
        assert!(is_hosted_unavailable(&error));
    }
}
