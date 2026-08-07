//! Google Gemini via `generativelanguage.googleapis.com`.
//!
//! Uses the `generateContent` surface deliberately. Google recommends the
//! newer Interactions API for new development, but two things this crate needs
//! live only on the older surface: `text/x.enum`, which returns a **bare**
//! token with no JSON wrapper, and `responseLogprobs`. Pinned to `v1beta` and
//! worth revisiting if either moves.
//!
//! Two findings from probing the reference key, both of which contradict the
//! documentation and are the reason nothing here is trusted without a probe:
//!
//! * `text/x.enum` works on every 2.5 and 3.5 model, and **fails on 2.0**.
//! * `responseLogprobs` is documented as generally available and is **enabled
//!   on no model** the reference key can reach — `2.0-flash`, `2.0-flash-lite`,
//!   `2.5-flash`, `2.5-flash-lite`, `3.5-flash` and `3.5-flash-lite` all
//!   refuse with `INVALID_ARGUMENT`.

use ovis_core::error::CoreResult;
use serde_json::{json, Value};

use super::{
    send_json, AdvertisedMetadata, Completion, CompletionRequest, Constraint, ModelInfo, Provider,
};

pub(super) async fn list_models(provider: &Provider) -> CoreResult<Vec<ModelInfo>> {
    let key = provider.key()?;
    let response = send_json(
        provider
            .http()
            .get(provider.url("/v1beta/models"))
            .query(&[("key", key), ("pageSize", "200")]),
        "list models",
    )
    .await?;

    let entries = response
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(entries
        .iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?;
            let id = name.strip_prefix("models/").unwrap_or(name).to_string();
            let methods: Vec<&str> = m
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            // An embedding model advertises `embedContent` and cannot generate.
            let is_embedding =
                methods.iter().any(|s| s.contains("embed")) || id.contains("embedding");
            if !is_embedding && !methods.contains(&"generateContent") {
                return None; // tts, imagen, and other non-chat surfaces
            }
            Some(ModelInfo {
                id,
                display_name: m
                    .get("displayName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                advertised: AdvertisedMetadata {
                    context_tokens: m
                        .get("inputTokenLimit")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32),
                    output_tokens: m
                        .get("outputTokenLimit")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32),
                    reasoning: m.get("thinking").and_then(Value::as_bool),
                    is_embedding,
                    description: m
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
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

    let mut generation = json!({ "maxOutputTokens": req.max_tokens });

    match &req.constraint {
        Constraint::OneOf(options) => {
            // The bare-token mode: output is `2`, not `{"score": 2}`. Only
            // available on this surface, and worth the pinning it costs.
            generation["responseMimeType"] = json!("text/x.enum");
            generation["responseSchema"] = json!({ "type": "STRING", "enum": options });
        }
        Constraint::Schema(schema) => {
            generation["responseMimeType"] = json!("application/json");
            generation["responseSchema"] = to_openapi_subset(schema);
        }
        Constraint::None => {}
    }
    if req.want_logprobs {
        generation["responseLogprobs"] = json!(true);
        generation["logprobs"] = json!(8);
    }
    if req.suppress_thinking {
        generation["thinkingConfig"] = thinking_config(&req.model);
    }

    let url = provider.url(&format!("/v1beta/models/{}:generateContent", req.model));
    let build = |generation: &Value| {
        json!({
            "contents": [{ "parts": [{ "text": content }] }],
            "generationConfig": generation,
        })
    };

    let first = send_json(
        provider
            .http()
            .post(&url)
            .query(&[("key", key)])
            .json(&build(&generation)),
        "completion",
    )
    .await;

    // The two generations disagree about how thinking is controlled, and the
    // model listing does not say which family a model belongs to. Rather than
    // hard-coding a name prefix that will be wrong at the next release, try
    // the other spelling once before giving up.
    let response = match first {
        Ok(response) => response,
        Err(err) if req.suppress_thinking && is_invalid_argument(&err) => {
            generation["thinkingConfig"] = alternate_thinking_config(&req.model);
            send_json(
                provider
                    .http()
                    .post(&url)
                    .query(&[("key", key)])
                    .json(&build(&generation)),
                "completion",
            )
            .await?
        }
        Err(err) => return Err(err),
    };

    let candidate = response
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .cloned()
        .unwrap_or(Value::Null);

    let text = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .and_then(|p| p.first())
        .and_then(|p| p.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let logprobs = candidate
        .get("logprobsResult")
        .and_then(|l| l.get("topCandidates"))
        .and_then(Value::as_array)
        .and_then(|steps| steps.first())
        .and_then(|step| step.get("candidates"))
        .and_then(Value::as_array)
        .map(|cands| {
            cands
                .iter()
                .filter_map(|c| {
                    Some((
                        c.get("token")?.as_str()?.to_string(),
                        c.get("logProbability")?.as_f64()?.exp(),
                    ))
                })
                .collect()
        });

    // `thoughtsTokenCount` means budget went to reasoning even when the
    // response looks clean.
    let had_thinking = response
        .get("usageMetadata")
        .and_then(|u| u.get("thoughtsTokenCount"))
        .and_then(Value::as_u64)
        .is_some_and(|n| n > 0);

    Ok(Completion {
        text,
        logprobs,
        had_thinking,
        finish_reason: candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt_tokens: response
            .get("usageMetadata")
            .and_then(|u| u.get("promptTokenCount"))
            .and_then(Value::as_u64)
            .map(|v| v as u32),
    })
}

/// How to ask a model not to spend its budget thinking.
///
/// The spelling changed between generations and the two are mutually
/// incompatible — measured against the live API, `gemini-2.5-flash-lite`
/// accepts `thinkingBudget: 0` and rejects `thinkingLevel`, while
/// `gemini-3.5-flash-lite` does the reverse and fails the whole request with
/// `INVALID_ARGUMENT` if given the wrong one. Getting this wrong makes a model
/// look incapable of constrained output when it is not.
fn thinking_config(model: &str) -> Value {
    if model.starts_with("gemini-3") {
        json!({ "thinkingLevel": "minimal" })
    } else {
        json!({ "thinkingBudget": 0 })
    }
}

/// The other spelling, for the retry.
fn alternate_thinking_config(model: &str) -> Value {
    if model.starts_with("gemini-3") {
        json!({ "thinkingBudget": 0 })
    } else {
        json!({ "thinkingLevel": "minimal" })
    }
}

fn is_invalid_argument(err: &ovis_core::error::CoreError) -> bool {
    let text = err.to_string();
    text.contains("400") || text.contains("INVALID_ARGUMENT") || text.contains("invalid argument")
}

/// Translate ordinary JSON Schema into the OpenAPI 3.0 subset `responseSchema`
/// accepts.
///
/// Two differences bite in practice. The subset **rejects**
/// `additionalProperties` with a 400 rather than ignoring it — measured
/// against the live API, and contrary to Vertex documentation that says
/// unsupported fields are ignored — and it expects `type` as an uppercase
/// OpenAPI type name rather than a lowercase JSON Schema one.
///
/// Translating here rather than at the call site means every caller writes
/// standard JSON Schema once and each provider adapts it.
fn to_openapi_subset(schema: &Value) -> Value {
    /// The fields the OpenAPI subset accepts. Anything else is dropped.
    const SUPPORTED: [&str; 14] = [
        "type",
        "format",
        "description",
        "nullable",
        "enum",
        "items",
        "properties",
        "required",
        "propertyOrdering",
        "anyOf",
        "minimum",
        "maximum",
        "minItems",
        "maxItems",
    ];

    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if !SUPPORTED.contains(&key.as_str()) {
                    continue;
                }
                let translated = match (key.as_str(), value) {
                    ("type", Value::String(t)) => Value::String(t.to_uppercase()),
                    ("properties", Value::Object(props)) => Value::Object(
                        props
                            .iter()
                            .map(|(k, v)| (k.clone(), to_openapi_subset(v)))
                            .collect(),
                    ),
                    // `enum` values are literals, not schemas — pass through.
                    ("enum", other) | ("required", other) => other.clone(),
                    (_, other) => to_openapi_subset(other),
                };
                out.insert(key.clone(), translated);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(to_openapi_subset).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;

    /// Measured against the live API: `additionalProperties` produces
    /// `Unknown name "additionalProperties" … Cannot find field` — a 400, not
    /// a silent ignore. Sending standard JSON Schema unchanged makes every
    /// schema-constrained call fail.
    #[test]
    fn unsupported_json_schema_keywords_are_stripped() {
        let standard = json!({
            "type": "object",
            "properties": { "answer": { "type": "string", "enum": ["0", "1"] } },
            "required": ["answer"],
            "additionalProperties": false,
            "$schema": "https://json-schema.org/draft/2020-12/schema"
        });
        let translated = to_openapi_subset(&standard);
        assert!(translated.get("additionalProperties").is_none());
        assert!(translated.get("$schema").is_none());
        // Everything meaningful survives.
        assert_eq!(translated["required"], json!(["answer"]));
        assert_eq!(
            translated["properties"]["answer"]["enum"],
            json!(["0", "1"])
        );
    }

    #[test]
    fn types_are_uppercased_for_the_openapi_dialect() {
        let translated = to_openapi_subset(&json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" },
                "count": { "type": "integer" },
                "tags": { "type": "array", "items": { "type": "string" } }
            }
        }));
        assert_eq!(translated["type"], "OBJECT");
        assert_eq!(translated["properties"]["answer"]["type"], "STRING");
        assert_eq!(translated["properties"]["count"]["type"], "INTEGER");
        assert_eq!(translated["properties"]["tags"]["items"]["type"], "STRING");
    }

    #[tokio::test]
    async fn embedding_and_non_chat_models_are_separated_from_judges() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v1beta/models"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "models": [
                    { "name": "models/gemini-2.5-flash-lite", "displayName": "Flash-Lite",
                      "inputTokenLimit": 1048576, "thinking": true,
                      "supportedGenerationMethods": ["generateContent", "countTokens"] },
                    { "name": "models/gemini-embedding-001",
                      "supportedGenerationMethods": ["embedContent"] },
                    { "name": "models/gemini-2.5-flash-preview-tts",
                      "supportedGenerationMethods": ["countTokens"] }
                ]
            })))
            .mount(&server)
            .await;

        let provider =
            Provider::new(ProviderKind::Gemini, Some(&server.uri()), Some("k".into())).unwrap();
        let models = provider.list_models().await.unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();

        assert!(ids.contains(&"gemini-2.5-flash-lite"));
        assert!(
            ids.contains(&"gemini-embedding-001"),
            "embedding models are listed, but flagged"
        );
        assert!(
            !ids.contains(&"gemini-2.5-flash-preview-tts"),
            "a model that cannot generateContent is not offered at all"
        );
        let flash = models.iter().find(|m| m.id.contains("flash-lite")).unwrap();
        assert_eq!(flash.advertised.reasoning, Some(true));
        assert_eq!(flash.advertised.context_tokens, Some(1048576));
        assert!(
            models
                .iter()
                .find(|m| m.id.contains("embedding"))
                .unwrap()
                .advertised
                .is_embedding
        );
    }

    #[tokio::test]
    async fn a_one_of_constraint_uses_bare_enum_mode_and_returns_an_unwrapped_token() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let g = &body["generationConfig"];
                assert_eq!(g["responseMimeType"], "text/x.enum");
                assert_eq!(g["responseSchema"]["type"], "STRING");
                assert_eq!(g["thinkingConfig"]["thinkingBudget"], 0);
                wiremock::ResponseTemplate::new(200).set_body_json(json!({
                    "candidates": [{ "content": { "parts": [{ "text": "2" }] } }],
                    "usageMetadata": { "promptTokenCount": 665 }
                }))
            })
            .mount(&server)
            .await;

        let provider =
            Provider::new(ProviderKind::Gemini, Some(&server.uri()), Some("k".into())).unwrap();
        let out = provider
            .complete(
                &CompletionRequest::new("gemini-2.5-flash-lite", "grade").constrained(
                    Constraint::OneOf(vec!["0".into(), "1".into(), "2".into(), "3".into()]),
                ),
            )
            .await
            .unwrap();
        // Bare token, no JSON to unwrap — the point of enum mode.
        assert_eq!(out.text, "2");
        assert_eq!(out.prompt_tokens, Some(665));
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::*;

    /// Measured live: 2.5 accepts `thinkingBudget` and rejects `thinkingLevel`;
    /// 3.5 does the reverse and fails the entire request either way.
    #[test]
    fn each_generation_gets_its_own_spelling_and_the_other_as_fallback() {
        assert_eq!(
            thinking_config("gemini-2.5-flash-lite")["thinkingBudget"],
            0
        );
        assert_eq!(
            thinking_config("gemini-3.5-flash-lite")["thinkingLevel"],
            "minimal"
        );
        // The retry must be the opposite spelling, never a repeat.
        assert_ne!(
            thinking_config("gemini-2.5-flash-lite"),
            alternate_thinking_config("gemini-2.5-flash-lite")
        );
        assert_eq!(
            alternate_thinking_config("gemini-3.5-flash-lite")["thinkingBudget"],
            0
        );
    }

    /// An unknown future model name must still reach a working call through
    /// the fallback rather than being locked to whichever guess came first.
    #[test]
    fn an_unrecognised_model_name_still_has_both_spellings_available() {
        let primary = thinking_config("gemini-9-something-new");
        let fallback = alternate_thinking_config("gemini-9-something-new");
        assert_ne!(primary, fallback);
        let spellings = [primary, fallback];
        assert!(spellings.iter().any(|c| c.get("thinkingBudget").is_some()));
        assert!(spellings.iter().any(|c| c.get("thinkingLevel").is_some()));
    }

    #[test]
    fn only_argument_errors_trigger_the_retry() {
        use ovis_core::error::CoreError;
        assert!(is_invalid_argument(&CoreError::Invalid(
            "completion: HTTP 400 Bad Request".into()
        )));
        assert!(!is_invalid_argument(&CoreError::Invalid(
            "completion: HTTP 404 Not Found".into()
        )));
        assert!(!is_invalid_argument(&CoreError::Invalid(
            "completion: HTTP 503 Service Unavailable".into()
        )));
    }
}
