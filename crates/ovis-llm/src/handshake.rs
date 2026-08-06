//! The capability handshake: verify what a model *does*, not what it claims.
//!
//! Everything else in this crate depends on this module being paranoid,
//! because every metadata source we checked was wrong about something on the
//! reference deployment, and because four documented provider behaviours
//! return **200 OK with plausible output** while silently ignoring the
//! constraint they were handed:
//!
//! | Provider | Silent failure |
//! |---|---|
//! | Ollama on Apple Silicon (MLX) | drops `format`/`grammar` entirely |
//! | llama.cpp | ignores the flat `response_format` its own README documents |
//! | vLLM | logprobs are post-mask, so masked tokens read `-inf` |
//! | OpenRouter | strips `response_format` without `require_parameters` |
//!
//! A probe that checked for HTTP 200, or for the presence of a JSON field,
//! would pass all four. So each probe here is **adversarial**: it asks for a
//! constrained answer using a prompt that actively invites an unconstrained
//! one, and passes only if the constraint won.

use ovis_core::error::CoreResult;
use serde::{Deserialize, Serialize};

use crate::provider::{CompletionRequest, Constraint, Provider};

/// Bumped when a probe's meaning changes, so a stored result from an older
/// build is visibly stale rather than silently trusted.
pub const PROBE_VERSION: i32 = 1;

/// The grades the enum probe constrains to. Deliberately the same alphabet the
/// judge uses, so the probe measures the real thing.
const GRADES: [&str; 4] = ["0", "1", "2", "3"];

/// A prompt engineered to produce prose from an unconstrained model. If the
/// answer still comes back as one of four digits, the constraint is real.
const TEMPTING_PROMPT: &str = "Explain in detail, in several sentences, how useful a website \
     navigation menu is as reference material. Discuss the trade-offs thoroughly before \
     concluding.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingChannel {
    /// No reasoning output observed.
    None,
    /// Reasoning output observed, and the provider's suppression worked.
    Suppressed,
    /// Reasoning output observed and could not be suppressed. The model will
    /// spend its token budget deliberating; usable only with a large budget.
    Unsuppressed,
}

/// What a model was measured to actually do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Capabilities {
    /// A one-of constraint is genuinely enforced.
    pub enum_enforced: bool,
    /// A JSON-schema constraint is genuinely enforced.
    pub schema_enforced: bool,
    /// A usable token distribution comes back.
    pub logprobs: bool,
    pub thinking_channel: ThinkingChannel,
    /// Populated when a probe failed, for display. Never a reason to retry —
    /// a model that cannot be constrained is not a judge.
    pub notes: Vec<String>,
    pub probe_version: i32,
    pub probed_at: chrono::DateTime<chrono::Utc>,
}

impl Capabilities {
    /// Whether this model may be assigned a judging role.
    ///
    /// One enforced constraint of either kind is the bar. Without it the
    /// model can emit arbitrary text in response to a document that asked it
    /// to, which is the whole threat.
    pub fn usable_as_judge(&self) -> bool {
        self.enum_enforced || self.schema_enforced
    }

    /// Whether scores from this model carry a confidence signal.
    ///
    /// Without logprobs a grade is a point estimate. Measured on the reference
    /// Gemini key, no model returns them — so confidence has to come from
    /// disagreement between two judges instead, which is a real cost, not a
    /// detail.
    pub fn calibratable(&self) -> bool {
        self.logprobs
    }

    /// The strongest constraint this model honours.
    pub fn best_constraint(&self, options: &[String]) -> Option<Constraint> {
        if self.enum_enforced {
            Some(Constraint::OneOf(options.to_vec()))
        } else if self.schema_enforced {
            Some(Constraint::Schema(serde_json::json!({
                "type": "object",
                "properties": { "answer": { "type": "string", "enum": options } },
                "required": ["answer"],
                "additionalProperties": false
            })))
        } else {
            None
        }
    }

    /// Whether an unsuppressible reasoning channel actually threatens judging.
    ///
    /// It does not, as long as *some* constraint is enforced: a decoder that
    /// may only emit `0`–`3` cannot emit a channel marker however much the
    /// model would like to. Measured on the reference Gemma build, which
    /// answers `\n<|channel>thought…` when unconstrained and a clean `2` under
    /// a grammar. Reporting that model as unusable would be wrong.
    pub fn thinking_blocks_judging(&self) -> bool {
        self.thinking_channel == ThinkingChannel::Unsuppressed && !self.usable_as_judge()
    }

    /// One line for the model picker, stated as findings rather than badges.
    pub fn summary(&self) -> String {
        let mark = |ok: bool| if ok { "✓" } else { "✗" };
        let thinking = match self.thinking_channel {
            ThinkingChannel::None => "no thinking channel",
            ThinkingChannel::Suppressed => "thinking channel — suppressed",
            ThinkingChannel::Unsuppressed if self.usable_as_judge() => {
                "reasons when unconstrained — moot under a constraint"
            }
            ThinkingChannel::Unsuppressed => "thinking channel — NOT suppressible",
        };
        format!(
            "enum {}  schema {}  logprobs {}  {thinking}",
            mark(self.enum_enforced),
            mark(self.schema_enforced),
            mark(self.logprobs),
        )
    }
}

/// Run every probe against one model.
///
/// Never returns `Err` for a capability being absent — absence is a finding.
/// An `Err` here means the endpoint itself is unreachable or the model does
/// not exist.
pub async fn probe(provider: &Provider, model: &str) -> CoreResult<Capabilities> {
    let mut notes = Vec::new();
    let grades: Vec<String> = GRADES.iter().map(|s| s.to_string()).collect();

    // --- 1. Is a one-of constraint enforced? ---
    let enum_result = provider
        .complete(
            &CompletionRequest::new(model, TEMPTING_PROMPT)
                .constrained(Constraint::OneOf(grades.clone()))
                .max_tokens(64),
        )
        .await;
    let enum_enforced = match &enum_result {
        Ok(completion) => {
            let answer = extract_answer(&completion.text);
            let ok = GRADES.contains(&answer.as_str());
            if !ok && !completion.text.is_empty() {
                notes.push(format!(
                    "one-of constraint not enforced: asked for a digit, got {:?}",
                    crate::provider::truncate(completion.text.trim(), 60)
                ));
            }
            ok
        }
        Err(err) => {
            notes.push(format!("one-of constraint rejected: {}", first_line(err)));
            false
        }
    };

    // --- 2. Is a JSON schema enforced? ---
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "answer": { "type": "string", "enum": GRADES } },
        "required": ["answer"],
        "additionalProperties": false
    });
    let schema_result = provider
        .complete(
            &CompletionRequest::new(model, TEMPTING_PROMPT)
                .constrained(Constraint::Schema(schema))
                .max_tokens(64),
        )
        .await;
    let schema_enforced = match &schema_result {
        Ok(completion) => {
            let ok = serde_json::from_str::<serde_json::Value>(completion.text.trim())
                .ok()
                .and_then(|v| {
                    let answer = v.get("answer")?.as_str()?.to_string();
                    Some(GRADES.contains(&answer.as_str()))
                })
                .unwrap_or(false);
            if !ok && !completion.text.is_empty() {
                notes.push(format!(
                    "schema not enforced: got {:?}",
                    crate::provider::truncate(completion.text.trim(), 60)
                ));
            }
            ok
        }
        Err(err) => {
            notes.push(format!("schema rejected: {}", first_line(err)));
            false
        }
    };

    // --- 3. Do logprobs come back? ---
    let logprob_result = provider
        .complete(
            &CompletionRequest::new(model, "Reply with the single digit 1.")
                .max_tokens(2)
                .logprobs(true),
        )
        .await;
    let logprobs = match &logprob_result {
        // A single candidate is a degenerate distribution and useless for
        // confidence — vLLM's post-mask logprobs look exactly like this.
        Ok(completion) => completion
            .logprobs
            .as_ref()
            .is_some_and(|l| l.len() > 1 && l.iter().any(|(_, p)| *p > 0.0)),
        Err(err) => {
            notes.push(format!("logprobs unavailable: {}", first_line(err)));
            false
        }
    };

    // --- 4. Is there a reasoning channel, and did suppression work? ---
    let suppressed = provider
        .complete(
            &CompletionRequest::new(model, "Reply with the single word: ready.").max_tokens(24),
        )
        .await;
    let thinking_channel = match &suppressed {
        Ok(completion) => {
            if !completion.had_thinking && !completion.text.trim().is_empty() {
                ThinkingChannel::None
            } else {
                // Something reasoned, or returned nothing at all. Retry with a
                // budget large enough to think *and* answer: if that produces
                // an answer, suppression is what failed.
                let retried = provider
                    .complete(
                        &CompletionRequest::new(model, "Reply with the single word: ready.")
                            .max_tokens(512),
                    )
                    .await;
                match retried {
                    Ok(second) if !second.text.trim().is_empty() => {
                        // Only alarming when nothing constrains the output.
                        // Under an enforced constraint the decoder cannot emit
                        // a channel marker at all.
                        if enum_enforced || schema_enforced {
                            notes.push(
                                "reasons when left unconstrained, but every judging call is \
                                 constrained, so this does not affect grading"
                                    .into(),
                            );
                        } else {
                            notes.push(
                                "emits a reasoning channel that cannot be suppressed, and no \
                                 constraint held to contain it"
                                    .into(),
                            );
                        }
                        ThinkingChannel::Unsuppressed
                    }
                    _ => ThinkingChannel::Suppressed,
                }
            }
        }
        Err(err) => {
            notes.push(format!("completion failed: {}", first_line(err)));
            ThinkingChannel::None
        }
    };

    if !enum_enforced && !schema_enforced {
        notes.push(
            "no constraint of either kind held, so this model cannot be used as a judge: a \
             document could make it emit arbitrary text"
                .into(),
        );
    }

    Ok(Capabilities {
        enum_enforced,
        schema_enforced,
        logprobs,
        thinking_channel,
        notes,
        probe_version: PROBE_VERSION,
        probed_at: chrono::Utc::now(),
    })
}

/// Pull the answer out of whatever shape came back — a bare enum token, or a
/// `{"answer": …}` object.
pub(crate) fn extract_answer(text: &str) -> String {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(answer) = value.get("answer").and_then(|v| v.as_str()) {
            return answer.trim().to_string();
        }
        if let Some(n) = value.get("answer").and_then(|v| v.as_i64()) {
            return n.to_string();
        }
    }
    trimmed.trim_matches('"').to_string()
}

/// A short, readable reason from a provider error.
///
/// Raw provider errors carry a truncated JSON body — `HTTP 404 Not Found: {`
/// — which is noise in a UI that shows one line per finding. Keep the status
/// and drop the body.
fn first_line(err: &ovis_core::error::CoreError) -> String {
    let text = err.to_string();
    let first = text.lines().next().unwrap_or_default();
    // Strip our own wrapper prefixes and any JSON body that follows. The
    // `invalid input:` prefix is the error *variant* leaking into prose.
    let first = first.strip_prefix("invalid input: ").unwrap_or(first);
    let without_wrapper = first
        .rsplit_once("completion: ")
        .map(|(_, rest)| rest)
        .unwrap_or(first);
    let without_body = without_wrapper
        .split_once(": {")
        .map(|(head, _)| head)
        .unwrap_or(without_wrapper);
    crate::provider::truncate(without_body.trim(), 120)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn caps(enum_ok: bool, schema_ok: bool, logprobs: bool) -> Capabilities {
        Capabilities {
            enum_enforced: enum_ok,
            schema_enforced: schema_ok,
            logprobs,
            thinking_channel: ThinkingChannel::None,
            notes: Vec::new(),
            probe_version: PROBE_VERSION,
            probed_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn a_model_that_honours_no_constraint_is_not_a_judge() {
        assert!(!caps(false, false, true).usable_as_judge());
        assert!(caps(true, false, false).usable_as_judge());
        assert!(caps(false, true, false).usable_as_judge());
    }

    #[test]
    fn the_strongest_available_constraint_is_chosen() {
        let options = vec!["0".to_string(), "1".to_string()];
        // Bare enum beats a JSON wrapper when both are available.
        assert!(matches!(
            caps(true, true, false).best_constraint(&options),
            Some(Constraint::OneOf(_))
        ));
        assert!(matches!(
            caps(false, true, false).best_constraint(&options),
            Some(Constraint::Schema(_))
        ));
        assert!(caps(false, false, false).best_constraint(&options).is_none());
    }

    #[test]
    fn answers_are_extracted_from_both_bare_and_wrapped_shapes() {
        assert_eq!(extract_answer("2"), "2");
        assert_eq!(extract_answer("  2 \n"), "2");
        assert_eq!(extract_answer(r#"{"answer":"2"}"#), "2");
        assert_eq!(extract_answer("{\n  \"answer\": \"3\"\n}"), "3");
        assert_eq!(extract_answer(r#"{"answer":1}"#), "1");
        assert_eq!(extract_answer("\"2\""), "2");
    }

    /// The MLX / OpenRouter trap: 200 OK, prose body, constraint discarded.
    /// A probe that trusted the status code would mark this model usable.
    #[tokio::test]
    async fn a_silently_ignored_constraint_fails_the_probe() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content":
                        "Well, a navigation menu is quite useful because it helps users..." },
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;

        let provider =
            Provider::new(ProviderKind::OpenAiCompatible, Some(&server.uri()), None).unwrap();
        let caps = probe(&provider, "pretend-model").await.unwrap();

        assert!(!caps.enum_enforced);
        assert!(!caps.schema_enforced);
        assert!(!caps.usable_as_judge());
        assert!(
            caps.notes.iter().any(|n| n.contains("not enforced")),
            "the finding must say what happened: {:?}",
            caps.notes
        );
        assert!(
            caps.notes.iter().any(|n| n.contains("cannot be used as a judge")),
            "{:?}",
            caps.notes
        );
    }

    #[tokio::test]
    async fn an_enforced_constraint_passes_despite_a_prose_inviting_prompt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(|request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                // Constrained calls answer in the constrained shape; the
                // logprob probe is unconstrained and answers plainly.
                let constrained = body.get("response_format").is_some();
                let content = if constrained { "{\"answer\":\"1\"}" } else { "1" };
                ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{ "message": { "content": content }, "finish_reason": "stop" }]
                }))
            })
            .mount(&server)
            .await;

        let provider =
            Provider::new(ProviderKind::OpenAiCompatible, Some(&server.uri()), None).unwrap();
        let caps = probe(&provider, "good-model").await.unwrap();
        assert!(caps.schema_enforced);
        assert!(caps.usable_as_judge());
        assert!(!caps.logprobs, "none were returned");
        assert!(!caps.calibratable());
    }

    /// vLLM returns post-mask logprobs: with a grammar applied, every masked
    /// token reads -inf and only the forced token has mass. That is not a
    /// distribution and must not be reported as one.
    #[tokio::test]
    async fn a_degenerate_single_candidate_distribution_is_not_counted_as_logprobs() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content": "1" },
                    "logprobs": { "content": [{ "top_logprobs": [
                        { "token": "1", "logprob": 0.0 }
                    ]}]},
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;

        let provider =
            Provider::new(ProviderKind::OpenAiCompatible, Some(&server.uri()), None).unwrap();
        let caps = probe(&provider, "vllm-ish").await.unwrap();
        assert!(!caps.logprobs, "one candidate carries no confidence information");
    }

    #[tokio::test]
    async fn a_thinking_model_that_returns_nothing_is_reported_not_silently_accepted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content": "", "reasoning_content": "Let me think about this..." },
                    "finish_reason": "length"
                }]
            })))
            .mount(&server)
            .await;

        let provider =
            Provider::new(ProviderKind::OpenAiCompatible, Some(&server.uri()), None).unwrap();
        let caps = probe(&provider, "thinker").await.unwrap();
        assert_eq!(caps.thinking_channel, ThinkingChannel::Suppressed);
        assert!(!caps.usable_as_judge(), "it never produced an answer");
    }

    #[test]
    fn the_summary_reads_as_findings() {
        let mut caps = caps(true, true, false);
        caps.thinking_channel = ThinkingChannel::Suppressed;
        let s = caps.summary();
        assert!(s.contains("enum ✓"));
        assert!(s.contains("logprobs ✗"));
        assert!(s.contains("thinking channel — suppressed"));
    }
}

#[cfg(test)]
mod thinking_semantics_tests {
    use super::*;

    fn caps(usable: bool, thinking: ThinkingChannel) -> Capabilities {
        Capabilities {
            enum_enforced: usable,
            schema_enforced: false,
            logprobs: false,
            thinking_channel: thinking,
            notes: Vec::new(),
            probe_version: PROBE_VERSION,
            probed_at: chrono::Utc::now(),
        }
    }

    /// The reference Gemma build answers `\n<|channel>thought…` when
    /// unconstrained and a clean digit under a GBNF grammar. Reporting it as
    /// unusable — or even as merely worrying — would be wrong, because every
    /// judging call carries a constraint.
    #[test]
    fn an_unsuppressible_channel_is_harmless_when_a_constraint_holds() {
        let constrained = caps(true, ThinkingChannel::Unsuppressed);
        assert!(!constrained.thinking_blocks_judging());
        assert!(constrained.usable_as_judge());
        assert!(
            constrained.summary().contains("moot under a constraint"),
            "{}",
            constrained.summary()
        );

        let loose = caps(false, ThinkingChannel::Unsuppressed);
        assert!(loose.thinking_blocks_judging());
        assert!(loose.summary().contains("NOT suppressible"));
    }
}

#[cfg(test)]
mod message_tests {
    use super::first_line;
    use ovis_core::error::CoreError;

    /// Probe findings are shown one per line in the UI; a truncated JSON body
    /// makes four near-identical failures unreadable.
    #[test]
    fn provider_errors_are_reduced_to_a_readable_reason() {
        let raw = CoreError::Invalid(
            "completion: HTTP 404 Not Found: {\n  \"error\": {\n    \"code\": 404".into(),
        );
        assert_eq!(first_line(&raw), "HTTP 404 Not Found");

        let with_message = CoreError::Invalid(
            "completion: HTTP 400 Bad Request: {\"error\":{\"message\":\"blah\"}}".into(),
        );
        assert_eq!(first_line(&with_message), "HTTP 400 Bad Request");

        // A plain message survives intact.
        let plain = CoreError::Invalid("connection refused".into());
        assert_eq!(first_line(&plain), "connection refused");
    }
}
