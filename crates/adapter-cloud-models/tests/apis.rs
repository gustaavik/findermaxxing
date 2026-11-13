//! Both protocols against the API doubles in `mock-cloud`: what is sent
//! (headers, schema, only the summary), how answers and failures are read.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use adapter_cloud_models::HttpOrganizationModel;
use application::ports::{
    CloudModelConfig, CloudProvider, ItemSummary, ModelError, OrganizationModel, OrganizeRequest, Secret,
};
use mock_cloud::MockCloud;
use serde_json::json;

fn request() -> OrganizeRequest {
    OrganizeRequest {
        folder_name: "Downloads".into(),
        items: vec![ItemSummary {
            id: 0,
            name: "scan0001.pdf".into(),
            kind: "pdf".into(),
            size: Some(1200),
            modified: Some("2026-03-14".into()),
            excerpt: Some("Lease agreement".into()),
        }],
    }
}

fn proposal_text() -> String {
    json!({"groups": [], "renames": [{"item_id": 0, "name": "Lease", "reason": "Clearer"}], "tags": []}).to_string()
}

fn anthropic(mock: &MockCloud, model: &str) -> CloudModelConfig {
    CloudModelConfig {
        provider: CloudProvider::Anthropic,
        base_url: format!("{}/anthropic", mock.base()),
        model: model.into(),
    }
}

fn openai(mock: &MockCloud) -> CloudModelConfig {
    CloudModelConfig {
        provider: CloudProvider::OpenAiCompatible,
        base_url: format!("{}/openai/v1", mock.base()),
        model: "llama3.3".into(),
    }
}

fn reply(mock: &MockCloud, status: u16, body: serde_json::Value) {
    mock.state().model_replies.push_back((status, body));
}

async fn ask(config: &CloudModelConfig) -> Result<application::ports::ModelProposal, ModelError> {
    HttpOrganizationModel::new().unwrap().propose(config, &Secret::new("sk-test".into()), &request()).await
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_requests_carry_the_schema_and_only_the_summary() {
    let mock = MockCloud::start().await;
    reply(
        &mock,
        200,
        json!({
            "stop_reason": "end_turn",
            "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": proposal_text()}],
        }),
    );
    let proposal = ask(&anthropic(&mock, "claude-opus-5-5")).await.unwrap();
    assert_eq!(proposal.renames[0].name, "Lease");

    let call = mock.state().model_calls[0].clone();
    assert_eq!(call.path, "/anthropic/v1/messages");
    assert_eq!(call.headers["x-api-key"], "sk-test");
    assert_eq!(call.headers["anthropic-version"], "2026-10-21");
    assert!(!call.headers.contains_key("anthropic-beta"), "fallbacks only on the real API");
    assert_eq!(call.body["model"], "claude-opus-5-5");
    assert_eq!(call.body["output_config"]["format"]["type"], "json_schema");
    assert!(call.body.get("tool_choice").is_none(), "forced tool use is refused by current models");
    let sent: serde_json::Value = serde_json::from_str(call.body["messages"][0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(sent["folder"], "Downloads");
    assert_eq!(sent["items"][0]["excerpt"], "Lease agreement");
}

#[tokio::test(flavor = "multi_thread")]
async fn refusals_rate_limits_and_bad_keys_are_told_apart() {
    let mock = MockCloud::start().await;
    let config = anthropic(&mock, "claude-opus-5-5");
    reply(&mock, 200, json!({"stop_reason": "refusal", "content": []}));
    assert!(matches!(ask(&config).await, Err(ModelError::Service(_))));
    reply(
        &mock,
        401,
        json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}}),
    );
    assert_eq!(ask(&config).await, Err(ModelError::Unauthorized));
    reply(&mock, 200, json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "{\"gro"}]}));
    assert!(matches!(ask(&config).await, Err(ModelError::BadAnswer(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn server_errors_are_retried() {
    let mock = MockCloud::start().await;
    reply(&mock, 529, json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}));
    reply(&mock, 200, json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": proposal_text()}]}));
    let started = std::time::Instant::now();
    assert!(ask(&anthropic(&mock, "claude-opus-5-5")).await.is_ok());
    assert_eq!(mock.state().model_calls.len(), 2);
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_compatible_servers_get_a_schema_and_a_second_chance_without_one() {
    let mock = MockCloud::start().await;
    reply(&mock, 200, json!({"choices": [{"message": {"content": format!("```json\n{}\n```", proposal_text())}}]}));
    let proposal = ask(&openai(&mock)).await.unwrap();
    assert_eq!(proposal.renames[0].item_id, 0);
    let call = mock.state().model_calls[0].clone();
    assert_eq!(call.path, "/openai/v1/chat/completions");
    assert_eq!(call.headers["authorization"], "Bearer sk-test");
    assert_eq!(call.body["response_format"]["type"], "json_schema");

    reply(&mock, 400, json!({"error": {"message": "unknown field: response_format"}}));
    reply(&mock, 200, json!({"choices": [{"message": {"content": proposal_text()}}]}));
    assert!(ask(&openai(&mock)).await.is_ok());
    let retry = mock.state().model_calls[2].clone();
    assert!(retry.body.get("response_format").is_none());
}
