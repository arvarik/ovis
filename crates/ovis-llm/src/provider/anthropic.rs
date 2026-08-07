//! Anthropic's Messages API.
//!
//! Structured output is genuinely strict here — grammar-constrained decoding,
//! generally available since January 2026 via `output_config.format`, no beta
//! header. That makes it a good `quality` judge.
//!
//! It exposes **no token logprobs at all**, so it can never produce a
//! calibrated score distribution. The handshake records that and the judge
//! degrades to a bare grade; it is not an error.

use ovis_core::error::CoreResult;
use serde_json::{json, Value};

use super::{
    send_json, AdvertisedMetadata, Completion, CompletionRequest, Constraint, ModelInfo, Provider,
};

const API_VERSION: &str = "2023-06-01";

pub(super) async fn list_models(provider: &Provider) -> CoreResult<Vec<ModelInfo>> {
    let key = provider.key()?;
    let response = send_json(
        provider
            .http()
            .get(provider.url("/v1/models"))
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .query(&[("limit", "1000")]),
        "list models",
    )
    .await?;

    let entries = response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(entries
        .iter()
        .filter_map(|m| {
            let capabilities = m.get("capabilities");
            Some(ModelInfo {
                id: m.get("id")?.as_str()?.to_string(),
                display_name: m
                    .get("display_name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                advertised: AdvertisedMetadata {
                    context_tokens: m
                        .get("max_input_tokens")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32),
                    output_tokens: m
                        .get("max_tokens")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32),
                    // Anthropic began shipping per-model capability metadata in
                    // 2026 — the only provider here that does.
                    reasoning: capabilities
                        .and_then(|c| c.get("thinking"))
                        .map(|t| !t.is_null()),
                    is_embedding: false,
                    description: None,
                },
            })
        })
        .collect())
}

pub(super) async fn complete(
    provider: &Provider,
    req: &CompletionRequest,
) -> CoreResult<Completion> {
    let key = provider.key()?;
    let content = crate::prompt::build(&req.instruction, req.document.as_deref());

    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens.max(1),
        "messages": [{ "role": "user", "content": content }],
    });

    match &req.constraint {
        Constraint::OneOf(options) => {
            body["output_config"] = json!({
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": { "answer": { "type": "string", "enum": options } },
                        "required": ["answer"],
                        "additionalProperties": false
                    }
                }
            });
        }
        Constraint::Schema(schema) => {
            body["output_config"] =
                json!({ "format": { "type": "json_schema", "schema": schema } });
        }
        Constraint::None => {}
    }

    let response = send_json(
        provider
            .http()
            .post(provider.url("/v1/messages"))
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .json(&body),
        "completion",
    )
    .await?;

    let text = response
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        })
        .and_then(|b| b.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let had_thinking = response
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("thinking"))
        });

    Ok(Completion {
        text,
        // Anthropic exposes no logprobs, at any tier, on any model.
        logprobs: None,
        had_thinking,
        finish_reason: response
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt_tokens: response
            .get("usage")
            .and_then(|u| u.get("input_tokens"))
            .and_then(Value::as_u64)
            .map(|v| v as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;

    #[tokio::test]
    async fn capability_metadata_is_read_where_the_provider_supplies_it() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v1/models"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": "claude-haiku-4-5", "display_name": "Claude Haiku 4.5",
                    "max_input_tokens": 200000, "max_tokens": 8192,
                    "capabilities": { "structured_outputs": { "supported": true },
                                      "thinking": { "types": { "enabled": true } } }
                }]
            })))
            .mount(&server)
            .await;

        let provider = Provider::new(
            ProviderKind::Anthropic,
            Some(&server.uri()),
            Some("k".into()),
        )
        .unwrap();
        let models = provider.list_models().await.unwrap();
        assert_eq!(models[0].advertised.context_tokens, Some(200000));
        assert_eq!(models[0].advertised.reasoning, Some(true));
    }

    /// Anthropic never returns logprobs. The judge has to degrade rather than
    /// treat their absence as a failure.
    #[tokio::test]
    async fn logprobs_are_always_absent_and_that_is_not_an_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "text", "text": "{\"answer\":\"2\"}" }],
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 120 }
            })))
            .mount(&server)
            .await;

        let provider = Provider::new(
            ProviderKind::Anthropic,
            Some(&server.uri()),
            Some("k".into()),
        )
        .unwrap();
        let out = provider
            .complete(&CompletionRequest::new("claude-haiku-4-5", "grade").logprobs(true))
            .await
            .unwrap();
        assert!(out.logprobs.is_none());
        assert!(out.text.contains("\"answer\""));
    }
}
