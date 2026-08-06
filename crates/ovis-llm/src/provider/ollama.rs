//! Ollama's native API.
//!
//! Listed separately from the OpenAI-compatible adapter because `/api/tags`
//! carries genuinely useful detail (parameter size, quantization, family) that
//! Ollama's OpenAI shim drops.
//!
//! Carries a known hazard: **on Apple Silicon with the MLX runner, Ollama
//! drops `format` entirely** and answers 200 with unconstrained text. Nothing
//! in the response distinguishes that from an enforced constraint, which is
//! precisely why the handshake tempts the model with a prose-inviting prompt
//! rather than checking a status code.

use ovis_core::error::CoreResult;
use serde_json::{json, Value};

use super::{
    looks_like_thinking, send_json, AdvertisedMetadata, Completion, CompletionRequest, Constraint,
    ModelInfo, Provider,
};

pub(super) async fn list_models(provider: &Provider) -> CoreResult<Vec<ModelInfo>> {
    let response = send_json(provider.http().get(provider.url("/api/tags")), "list models").await?;

    let entries = response
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(entries
        .iter()
        .filter_map(|m| {
            let id = m.get("name")?.as_str()?.to_string();
            let details = m.get("details");
            let family = details
                .and_then(|d| d.get("family"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let parameter_size = details
                .and_then(|d| d.get("parameter_size"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let quantization = details
                .and_then(|d| d.get("quantization_level"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let is_embedding = id.contains("embed") || family.contains("bert");
            let description = [parameter_size, quantization]
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ");
            Some(ModelInfo {
                id,
                display_name: None,
                advertised: AdvertisedMetadata {
                    is_embedding,
                    description: (!description.is_empty()).then_some(description),
                    ..AdvertisedMetadata::default()
                },
            })
        })
        .collect())
}

pub(super) async fn complete(
    provider: &Provider,
    req: &CompletionRequest,
) -> CoreResult<Completion> {
    let content = crate::prompt::build(&req.instruction, req.document.as_deref());

    let mut body = json!({
        "model": req.model,
        "messages": [{ "role": "user", "content": content }],
        "stream": false,
        "options": { "temperature": 0, "num_predict": req.max_tokens },
    });

    match &req.constraint {
        Constraint::OneOf(options) => {
            body["format"] = json!({
                "type": "object",
                "properties": { "answer": { "type": "string", "enum": options } },
                "required": ["answer"]
            });
        }
        Constraint::Schema(schema) => {
            body["format"] = schema.clone();
        }
        Constraint::None => {}
    }
    if req.suppress_thinking {
        body["think"] = json!(false);
    }

    let response = send_json(
        provider.http().post(provider.url("/api/chat")).json(&body),
        "completion",
    )
    .await?;

    let message = response.get("message").cloned().unwrap_or(Value::Null);
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let thinking = message
        .get("thinking")
        .and_then(Value::as_str)
        .unwrap_or_default();

    Ok(Completion {
        text: text.clone(),
        // Ollama exposes no logprobs on /api/chat.
        logprobs: None,
        had_thinking: !thinking.is_empty() || looks_like_thinking(&text),
        finish_reason: response
            .get("done_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt_tokens: response
            .get("prompt_eval_count")
            .and_then(Value::as_u64)
            .map(|v| v as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;

    #[tokio::test]
    async fn tag_details_become_a_readable_description() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/tags"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "models": [
                    { "name": "qwen3:8b", "details": { "family": "qwen3",
                      "parameter_size": "8.2B", "quantization_level": "Q4_K_M" } },
                    { "name": "nomic-embed-text:latest", "details": { "family": "bert" } }
                ]
            })))
            .mount(&server)
            .await;

        let provider = Provider::new(ProviderKind::Ollama, Some(&server.uri()), None).unwrap();
        let models = provider.list_models().await.unwrap();
        let chat = models.iter().find(|m| m.id == "qwen3:8b").unwrap();
        assert_eq!(chat.advertised.description.as_deref(), Some("8.2B · Q4_K_M"));
        assert!(!chat.advertised.is_embedding);
        assert!(models.iter().find(|m| m.id.contains("nomic")).unwrap().advertised.is_embedding);
    }

    /// The MLX trap: 200 OK, no error, and the `format` constraint quietly
    /// discarded. Only the *content* of the reply reveals it, which is why the
    /// handshake looks at the answer rather than the status.
    #[tokio::test]
    async fn an_ignored_format_constraint_is_indistinguishable_from_success_at_the_transport() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/chat"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "message": { "role": "assistant",
                             "content": "Well, a navigation menu is quite useful because..." },
                "done_reason": "stop"
            })))
            .mount(&server)
            .await;

        let provider = Provider::new(ProviderKind::Ollama, Some(&server.uri()), None).unwrap();
        let out = provider
            .complete(
                &CompletionRequest::new("qwen3:8b", "grade")
                    .constrained(Constraint::OneOf(vec!["0".into(), "1".into()])),
            )
            .await
            .unwrap();
        // The call succeeded; the constraint did not hold. Nothing at this
        // layer can tell — that judgement belongs to the handshake.
        assert!(out.text.starts_with("Well,"));
    }
}
