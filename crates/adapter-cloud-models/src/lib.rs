//! `OrganizationModel` (ADR 0029): the user's own cloud model, asked about
//! one opted-in folder at a time.
//!
//! - **Anthropic** (Messages API, raw HTTP: there is no Rust SDK): the
//!   answer is constrained to a JSON schema with structured outputs
//!   (`output_config.format`); current models refuse forced tool use.
//!   Models that support it get `fallbacks: "default"`, so a safety
//!   classifier's refusal is retried server-side on a suitable model.
//! - **OpenAI-compatible** (`/chat/completions`): `response_format` with the
//!   same schema; servers that don't support it are asked again with the
//!   schema in the prompt only.
//!
//! Rate limits and server errors are retried twice, honouring `retry-after`.

mod prompt;

use std::time::Duration;

use application::ports::{
    CloudModelConfig, CloudProvider, ModelError, ModelProposal, OrganizationModel, OrganizeRequest, Secret,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use reqwest::StatusCode;
use serde_json::{Value, json};

pub use prompt::{parse_proposal, proposal_schema, request_json};

const ANTHROPIC_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Model families that accept `fallbacks: "default"` on the Claude API.
const FALLBACK_MODELS: &[&str] = &["claude-fable-5", "claude-opus-5", "claude-sonnet-5-5"];
const ANTHROPIC_HOST: &str = "api.anthropic.com";
/// Room for the whole proposal of a large folder.
const MAX_OUTPUT_TOKENS: u32 = 16_000;
/// Organizing is judgement, not deep reasoning: the model's middle setting.
const EFFORT: &str = "medium";
/// A thinking model organizing 200 items can take a while.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const RETRIES: u32 = 2;
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const SCHEMA_NAME: &str = "organization_proposal";

#[derive(Debug, Clone)]
pub struct HttpOrganizationModel {
    client: reqwest::Client,
}

impl HttpOrganizationModel {
    /// # Errors
    /// The HTTP client couldn't be built (no TLS roots).
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self { client: reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build()? })
    }
}

impl OrganizationModel for HttpOrganizationModel {
    fn propose(
        &self,
        config: &CloudModelConfig,
        key: &Secret,
        request: &OrganizeRequest,
    ) -> BoxFuture<'static, Result<ModelProposal, ModelError>> {
        let (client, config, key, request) = (self.client.clone(), config.clone(), key.clone(), request.clone());
        async move {
            match config.provider {
                CloudProvider::Anthropic => anthropic(&client, &config, &key, &request).await,
                CloudProvider::OpenAiCompatible => openai(&client, &config, &key, &request).await,
            }
        }
        .boxed()
    }
}

fn endpoint(base: &str, path: &str) -> String {
    format!("{}/{path}", base.trim_end_matches('/'))
}

fn uses_fallbacks(config: &CloudModelConfig) -> bool {
    let official = reqwest::Url::parse(&config.base_url).is_ok_and(|u| u.host_str() == Some(ANTHROPIC_HOST));
    official && FALLBACK_MODELS.iter().any(|family| config.model.starts_with(family))
}

async fn anthropic(
    client: &reqwest::Client,
    config: &CloudModelConfig,
    key: &Secret,
    request: &OrganizeRequest,
) -> Result<ModelProposal, ModelError> {
    let mut body = json!({
        "model": config.model,
        "max_tokens": MAX_OUTPUT_TOKENS,
        "system": prompt::INSTRUCTIONS,
        "messages": [{"role": "user", "content": request_json(request).to_string()}],
        "output_config": {"effort": EFFORT, "format": {"type": "json_schema", "schema": proposal_schema()}},
    });
    let fallbacks = uses_fallbacks(config);
    if fallbacks {
        body["fallbacks"] = json!("default");
    }
    let answer = send(|| {
        let call = client
            .post(endpoint(&config.base_url, "v1/messages"))
            .header("x-api-key", key.expose())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body);
        if fallbacks { call.header("anthropic-beta", FALLBACK_BETA) } else { call }
    })
    .await?;
    match answer["stop_reason"].as_str() {
        Some("refusal") => return Err(ModelError::Service("the model declined to answer".into())),
        Some("max_tokens") => return Err(ModelError::BadAnswer("the answer was cut off; try a smaller folder".into())),
        _ => {}
    }
    // Thinking blocks may come first; the answer is the text.
    let text: String = answer["content"]
        .as_array()
        .map(|blocks| blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect())
        .unwrap_or_default();
    parse_proposal(&text)
}

async fn openai(
    client: &reqwest::Client,
    config: &CloudModelConfig,
    key: &Secret,
    request: &OrganizeRequest,
) -> Result<ModelProposal, ModelError> {
    let messages = json!([
        {"role": "system", "content": format!("{}\n\nAnswer with JSON only, matching this schema:\n{}", prompt::INSTRUCTIONS, proposal_schema())},
        {"role": "user", "content": request_json(request).to_string()},
    ]);
    let structured = json!({
        "model": config.model,
        "messages": messages,
        "response_format": {"type": "json_schema", "json_schema": {"name": SCHEMA_NAME, "strict": true, "schema": proposal_schema()}},
    });
    let call = |body: Value| {
        let builder = client.post(endpoint(&config.base_url, "chat/completions")).json(&body);
        if key.expose().is_empty() { builder } else { builder.bearer_auth(key.expose()) }
    };
    let answer = match send(|| call(structured.clone())).await {
        // Some servers don't know `response_format`: the prompt alone still asks for the schema.
        Err(ModelError::Service(message)) if message.contains("response_format") => {
            send(|| call(json!({"model": config.model, "messages": messages}))).await?
        }
        other => other?,
    };
    let text = answer["choices"][0]["message"]["content"].as_str().unwrap_or_default();
    parse_proposal(text)
}

/// Sends with retries on 429, 5xx and 529; maps the rest to [`ModelError`].
async fn send(build: impl Fn() -> reqwest::RequestBuilder) -> Result<Value, ModelError> {
    let mut attempt = 0;
    loop {
        let response = build().send().await.map_err(|e| ModelError::Network(e.without_url().to_string()))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs);
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(body);
        }
        let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
        if retryable && attempt < RETRIES {
            attempt += 1;
            tokio::time::sleep(retry_after.unwrap_or(DEFAULT_RETRY_DELAY).min(MAX_RETRY_DELAY)).await;
            continue;
        }
        return Err(error_for(status, &body));
    }
}

fn error_for(status: StatusCode, body: &Value) -> ModelError {
    let message = body["error"]["message"].as_str().or_else(|| body["error"].as_str()).unwrap_or_default().to_owned();
    match status.as_u16() {
        401 | 403 => ModelError::Unauthorized,
        429 | 529 => ModelError::RateLimited,
        _ if message.is_empty() => ModelError::Service(format!("HTTP {status}")),
        _ => ModelError::Service(message),
    }
}
