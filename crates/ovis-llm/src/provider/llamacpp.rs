//! llama.cpp's own server.
//!
//! Kept separate from the OpenAI-compatible adapter because the useful path is
//! `/completion` rather than `/v1/chat/completions`. Two capabilities live
//! only there, and both were needed to get a usable answer out of the
//! reference deployment's Gemma build:
//!
//! * **GBNF grammars.** `root ::= "0" | "1" | "2" | "3"` is a genuine decoder
//!   constraint — the strongest available anywhere, and stronger than any
//!   hosted provider offers for a bare token.
//! * **Assistant prefill.** A reasoning model opens its turn with a channel
//!   sentinel and spends the entire token budget deliberating; measured on the
//!   reference server, a 200-token request returned `finish_reason: length`
//!   and an *empty* answer. Prefilling the assistant turn past the sentinel
//!   is what makes the model answer immediately.
//!
//! Together those two are why the local box, not the hosted API, is the tier
//! that can return a calibrated score.

use ovis_core::error::CoreResult;
use serde_json::{json, Value};

use super::{
    looks_like_thinking, send_json, AdvertisedMetadata, Completion, CompletionRequest, Constraint,
    ModelInfo, Provider,
};

/// Prefix that closes the reasoning channel and opens the final one. Matches
/// the sentinel the reference build emits; harmless on models that do not use
/// channels, which simply see it as the start of their own turn.
const FINAL_CHANNEL_PREFILL: &str = "<|channel>final<|message>";

pub(super) async fn list_models(provider: &Provider) -> CoreResult<Vec<ModelInfo>> {
    let response = send_json(
        provider.authed(provider.http().get(provider.url("/v1/models"))),
        "list models",
    )
    .await?;

    // llama.cpp answers with `models`; some builds mirror OpenAI's `data`.
    let entries = response
        .get("models")
        .or_else(|| response.get("data"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(entries
        .iter()
        .filter_map(|m| {
            // The id is a filesystem path to a GGUF; show the basename.
            let raw = m
                .get("id")
                .or_else(|| m.get("name"))
                .or_else(|| m.get("model"))?
                .as_str()?;
            let display = raw
                .rsplit('/')
                .next()
                .unwrap_or(raw)
                .trim_end_matches(".gguf")
                .to_string();
            Some(ModelInfo {
                id: raw.to_string(),
                display_name: Some(display),
                advertised: AdvertisedMetadata {
                    context_tokens: m
                        .get("n_ctx")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32),
                    // llama.cpp reports `capabilities: ["completion"]` and
                    // nothing else. Everything meaningful comes from the probe.
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

    // Gemma-style turn markers. Chosen because the reference deployment serves
    // Gemma; other templates tolerate the extra tokens as ordinary text.
    let mut prompt = format!("<start_of_turn>user\n{content}<end_of_turn>\n<start_of_turn>model\n");
    if req.suppress_thinking {
        prompt.push_str(FINAL_CHANNEL_PREFILL);
    }

    let mut body = json!({
        "prompt": prompt,
        "n_predict": req.max_tokens,
        "temperature": 0,
        "cache_prompt": true,
    });

    match &req.constraint {
        Constraint::OneOf(options) => {
            body["grammar"] = json!(gbnf_alternation(options));
        }
        Constraint::Schema(schema) => {
            // llama.cpp compiles a JSON schema to a grammar itself.
            body["json_schema"] = schema.clone();
        }
        Constraint::None => {}
    }
    if req.want_logprobs {
        body["n_probs"] = json!(8);
    }

    let response = send_json(
        provider.authed(provider.http().post(provider.url("/completion")).json(&body)),
        "completion",
    )
    .await?;

    let text = response
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Current builds report `top_logprobs` with `logprob` values; older ones
    // used `top_probs`/`probs` with a linear `prob`. Read whichever is there —
    // the field moved between releases, and a missing distribution is a
    // capability finding rather than an error.
    //
    // Note these are the model's **pre-mask** preferences: with a grammar
    // applied, the emitted token is forced but the distribution still shows
    // what the model wanted. That is more informative than a post-mask
    // distribution, and it is why the judge filters to grade tokens and
    // renormalizes rather than reading the top entry.
    let logprobs = response
        .get("completion_probabilities")
        .and_then(Value::as_array)
        .and_then(|steps| steps.first())
        .and_then(|step| {
            step.get("top_logprobs")
                .or_else(|| step.get("top_probs"))
                .or_else(|| step.get("probs"))
                .and_then(Value::as_array)
        })
        .map(|tops| {
            tops.iter()
                .filter_map(|t| {
                    let token = t
                        .get("token")
                        .or_else(|| t.get("tok_str"))?
                        .as_str()?
                        .to_string();
                    let p = t
                        .get("prob")
                        .and_then(Value::as_f64)
                        .or_else(|| t.get("logprob").and_then(Value::as_f64).map(f64::exp))?;
                    Some((token, p))
                })
                .collect()
        });

    Ok(Completion {
        text: text.clone(),
        logprobs,
        // With the prefill in place a sentinel in the output means the
        // suppression did not take.
        had_thinking: looks_like_thinking(&text),
        finish_reason: response
            .get("stop_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt_tokens: response
            .get("timings")
            .and_then(|t| t.get("prompt_n"))
            .and_then(Value::as_u64)
            .map(|v| v as u32),
    })
}

/// A GBNF rule matching exactly one of the options.
///
/// This is a real decoder constraint: tokens outside the alternation are
/// masked, so the model cannot emit prose however the prompt is manipulated.
fn gbnf_alternation(options: &[String]) -> String {
    let alts: Vec<String> = options.iter().map(|o| format!("{:?}", o)).collect();
    format!("root ::= ({})", alts.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;

    #[test]
    fn grammar_quotes_and_escapes_every_option() {
        let g = gbnf_alternation(&["0".into(), "1".into(), "2".into(), "3".into()]);
        assert_eq!(g, r#"root ::= ("0" | "1" | "2" | "3")"#);
        // A quote inside an option must not break out of the rule.
        let g = gbnf_alternation(&["a\"b".into()]);
        assert!(g.contains(r#""a\"b""#), "{g}");
    }

    /// Measured on the reference server: without the prefill a 200-token
    /// request came back `finish_reason: length` with empty content, the whole
    /// budget spent in `reasoning_content`.
    #[test]
    fn suppressing_thinking_prefills_past_the_channel_sentinel() {
        assert!(FINAL_CHANNEL_PREFILL.starts_with("<|channel>"));
        assert!(FINAL_CHANNEL_PREFILL.ends_with("<|message>"));
    }

    #[tokio::test]
    async fn the_grammar_is_sent_and_the_bare_answer_is_returned() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/completion"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                // Assert the two llama.cpp-specific affordances are present.
                assert_eq!(body["grammar"], r#"root ::= ("yes" | "no")"#);
                assert!(
                    body["prompt"].as_str().unwrap().ends_with(FINAL_CHANNEL_PREFILL),
                    "the assistant turn must be prefilled past the channel"
                );
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(json!({ "content": "no", "timings": { "prompt_n": 698 } }))
            })
            .mount(&server)
            .await;

        let provider = Provider::new(ProviderKind::LlamaCpp, Some(&server.uri()), None).unwrap();
        let out = provider
            .complete(
                &CompletionRequest::new("gemma", "is this useful?")
                    .constrained(Constraint::OneOf(vec!["yes".into(), "no".into()])),
            )
            .await
            .unwrap();
        assert_eq!(out.text, "no");
        assert!(!out.had_thinking);
        assert_eq!(out.prompt_tokens, Some(698));
    }

    /// `llama-server --api-key` exists, so the key has to reach the wire when
    /// one is configured — and has to stay off it when one is not, because an
    /// unauthenticated server rejects a bearer header it did not ask for.
    #[tokio::test]
    async fn a_key_is_sent_only_when_one_is_configured() {
        for key in [Some("sk-local"), None] {
            let server = wiremock::MockServer::start().await;
            let expected = key.map(|k| format!("Bearer {k}"));
            wiremock::Mock::given(wiremock::matchers::method("GET"))
                .and(wiremock::matchers::path("/v1/models"))
                .respond_with(move |request: &wiremock::Request| {
                    let sent = request
                        .headers
                        .get("authorization")
                        .map(|v| v.to_str().unwrap().to_string());
                    assert_eq!(sent, expected, "authorization header for key {key:?}");
                    wiremock::ResponseTemplate::new(200).set_body_json(json!({ "models": [] }))
                })
                .mount(&server)
                .await;

            let provider =
                Provider::new(ProviderKind::LlamaCpp, Some(&server.uri()), key.map(String::from))
                    .unwrap();
            provider.list_models().await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_gguf_path_is_shown_by_basename() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v1/models"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "models": [{
                    "name": "/models/gemma-4-12B-it-Q5_K_M.gguf",
                    "model": "/models/gemma-4-12B-it-Q5_K_M.gguf",
                    "capabilities": ["completion"]
                }]
            })))
            .mount(&server)
            .await;

        let provider = Provider::new(ProviderKind::LlamaCpp, Some(&server.uri()), None).unwrap();
        let models = provider.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].display_name.as_deref(), Some("gemma-4-12B-it-Q5_K_M"));
        assert!(models[0].id.starts_with("/models/"));
    }
}
