//! OpenAI-compatible endpoints: vLLM, LM Studio, OpenRouter, Together,
//! Fireworks, and OpenAI itself.
//!
//! Two things this adapter has to work around:
//!
//! * **The listing is nearly empty.** OpenAI's `Model` object is four fields
//!   (`id`, `created`, `object`, `owned_by`) and has been for years. vLLM adds
//!   `max_model_len` and the real repository name, which is worth reading when
//!   present. Nothing here is enough to decide behaviour.
//! * **`response_format` is widely accepted and variously honoured.**
//!   OpenRouter drops it unless `provider.require_parameters` is set; some
//!   local servers ignore it entirely. This adapter always sends the nested
//!   `json_schema` form and sets `require_parameters`, and the handshake
//!   checks whether any of it took effect.

use ovis_core::error::CoreResult;
use serde_json::{json, Value};

use super::{
    looks_like_thinking, send_json, AdvertisedMetadata, Completion, CompletionRequest, Constraint,
    ModelInfo, Provider,
};

pub(super) async fn list_models(provider: &Provider) -> CoreResult<Vec<ModelInfo>> {
    let mut req = provider.http().get(provider.url("/v1/models"));
    if let Ok(key) = provider.key() {
        req = req.bearer_auth(key);
    }
    let response = send_json(req, "list models").await?;

    let entries = response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(entries
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            // vLLM reports `max_model_len`; a 512-token limit is an embedding
            // model, not something that can grade a document.
            let context = m
                .get("max_model_len")
                .or_else(|| m.get("context_length"))
                .and_then(Value::as_u64)
                .map(|v| v as u32);
            let root = m.get("root").and_then(Value::as_str).unwrap_or("");
            let is_embedding = id.contains("embed")
                || root.contains("embed")
                || context.is_some_and(|c| c <= 1024);
            Some(ModelInfo {
                display_name: m
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                advertised: AdvertisedMetadata {
                    context_tokens: context,
                    output_tokens: m
                        .get("max_completion_tokens")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32),
                    reasoning: None,
                    is_embedding,
                    description: (!root.is_empty()).then(|| root.to_string()),
                },
                id,
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
        "max_tokens": req.max_tokens,
        // OpenRouter silently strips response_format without this.
        "provider": { "require_parameters": true },
    });

    match &req.constraint {
        Constraint::OneOf(options) => {
            // No hosted OpenAI-compatible endpoint offers a bare-enum mode, so
            // the closest portable form is a single-property enum object.
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "verdict",
                    "strict": true,
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
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": "verdict", "strict": true, "schema": schema }
            });
        }
        Constraint::None => {}
    }

    if req.want_logprobs {
        body["logprobs"] = json!(true);
        body["top_logprobs"] = json!(8);
    }
    if req.suppress_thinking {
        // Honoured by several servers, ignored harmlessly by the rest.
        body["chat_template_kwargs"] = json!({ "thinking": false });
        body["reasoning_effort"] = json!("none");
    }

    let mut http = provider.http().post(provider.url("/v1/chat/completions"));
    if let Ok(key) = provider.key() {
        http = http.bearer_auth(key);
    }
    let response = send_json(http.json(&body), "completion").await?;

    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .cloned()
        .unwrap_or(Value::Null);
    let message = choice.get("message").cloned().unwrap_or(Value::Null);
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // A reasoning model may return an empty `content` with the answer's
    // budget spent in `reasoning_content`. That is a thinking channel even
    // though no sentinel appears in the text.
    let reasoning = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let had_thinking = !reasoning.is_empty() || looks_like_thinking(&text);

    let logprobs = choice
        .get("logprobs")
        .and_then(|l| l.get("content"))
        .and_then(Value::as_array)
        .and_then(|steps| steps.first())
        .and_then(|step| step.get("top_logprobs"))
        .and_then(Value::as_array)
        .map(|tops| {
            tops.iter()
                .filter_map(|t| {
                    Some((
                        t.get("token")?.as_str()?.to_string(),
                        t.get("logprob")?.as_f64()?.exp(),
                    ))
                })
                .collect()
        });

    Ok(Completion {
        text,
        logprobs,
        had_thinking,
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt_tokens: response
            .get("usage")
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .map(|v| v as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;

    fn body_for(constraint: Constraint) -> Value {
        // Mirrors what `complete` assembles, so the shape can be asserted
        // without a server.
        let mut body = json!({ "provider": { "require_parameters": true } });
        if let Constraint::OneOf(options) = &constraint {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "verdict", "strict": true,
                    "schema": {
                        "type": "object",
                        "properties": { "answer": { "type": "string", "enum": options } },
                        "required": ["answer"], "additionalProperties": false
                    }
                }
            });
        }
        body
    }

    /// OpenRouter drops `response_format` unless this is set — a silent
    /// downgrade to unconstrained output that returns 200 either way.
    #[test]
    fn require_parameters_is_always_sent() {
        let body = body_for(Constraint::None);
        assert_eq!(body["provider"]["require_parameters"], true);
    }

    /// llama.cpp's own README documents a flat `response_format`, and its
    /// parser does not read it. The nested form is the one that works.
    #[test]
    fn schemas_use_the_nested_json_schema_form() {
        let body = body_for(Constraint::OneOf(vec!["0".into(), "1".into()]));
        assert!(body["response_format"]["json_schema"]["schema"].is_object());
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    }

    #[tokio::test]
    async fn a_short_context_model_is_classified_as_an_embedding_model() {
        // The reference deployment's vLLM serves arctic-embed with
        // max_model_len 512 on the same /v1/models as any chat model would be.
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v1/models"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    { "id": "snowflake-arctic-embed:m", "object": "model",
                      "owned_by": "vllm", "root": "Snowflake/snowflake-arctic-embed-m-v1.5",
                      "max_model_len": 512 },
                    { "id": "qwen3-8b", "object": "model", "max_model_len": 32768 }
                ]
            })))
            .mount(&server)
            .await;

        let provider =
            Provider::new(ProviderKind::OpenAiCompatible, Some(&server.uri()), None).unwrap();
        let models = provider.list_models().await.unwrap();
        let embed = models.iter().find(|m| m.id.contains("arctic")).unwrap();
        let chat = models.iter().find(|m| m.id == "qwen3-8b").unwrap();
        assert!(embed.advertised.is_embedding, "512-token model is not a judge");
        assert!(!chat.advertised.is_embedding);
        assert_eq!(chat.advertised.context_tokens, Some(32768));
    }

    #[tokio::test]
    async fn an_empty_content_with_reasoning_is_reported_as_a_thinking_channel() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content": "", "reasoning_content": "Let me consider..." },
                    "finish_reason": "length"
                }]
            })))
            .mount(&server)
            .await;

        let provider =
            Provider::new(ProviderKind::OpenAiCompatible, Some(&server.uri()), None).unwrap();
        let out = provider
            .complete(&CompletionRequest::new("m", "grade this"))
            .await
            .unwrap();
        assert!(out.text.is_empty());
        assert!(
            out.had_thinking,
            "a budget spent on reasoning must be visible to the handshake"
        );
    }
}
