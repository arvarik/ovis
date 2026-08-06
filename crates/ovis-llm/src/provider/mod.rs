//! One provider, five wire dialects.
//!
//! Enum dispatch rather than a trait object: the set of providers is fixed and
//! small, every variant needs a different request *shape* rather than a
//! different implementation of the same shape, and a `match` keeps the
//! differences visible in one file instead of scattered across five.

mod anthropic;
mod gemini;
mod llamacpp;
mod ollama;
mod openai;

use std::time::Duration;

use ovis_core::error::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};

/// How to talk to an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// vLLM, LM Studio, OpenRouter, Together, and anything else that speaks
    /// `/v1/models` + `/v1/chat/completions`.
    OpenAiCompatible,
    Gemini,
    Anthropic,
    Ollama,
    /// llama.cpp's own server. Distinct from OpenAI-compatible because the
    /// useful path is `/completion` with a GBNF grammar and an assistant
    /// prefill — which is the only way to get a clean answer out of a model
    /// that emits a reasoning channel.
    LlamaCpp,
}

impl ProviderKind {
    pub fn parse(raw: &str) -> CoreResult<Self> {
        match raw {
            "openai_compatible" => Ok(Self::OpenAiCompatible),
            "gemini" => Ok(Self::Gemini),
            "anthropic" => Ok(Self::Anthropic),
            "ollama" => Ok(Self::Ollama),
            "llamacpp" => Ok(Self::LlamaCpp),
            other => Err(CoreError::Invalid(format!(
                "unknown provider kind '{other}'; expected one of openai_compatible, \
                 gemini, anthropic, ollama, llamacpp"
            ))),
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
            Self::Gemini => "gemini",
            Self::Anthropic => "anthropic",
            Self::Ollama => "ollama",
            Self::LlamaCpp => "llamacpp",
        }
    }

    /// Whether this kind needs an API key to be usable at all. Self-hosted
    /// endpoints do not, which is what makes "point it at your own box" a
    /// one-field setup.
    pub fn requires_key(self) -> bool {
        matches!(self, Self::Gemini | Self::Anthropic)
    }

    /// The default endpoint, where the provider has a single well-known one.
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Gemini => Some("https://generativelanguage.googleapis.com"),
            Self::Anthropic => Some("https://api.anthropic.com"),
            Self::Ollama => Some("http://localhost:11434"),
            _ => None,
        }
    }
}

/// What the provider's own listing says about a model.
///
/// Advisory only. Nothing here may gate behaviour — that is
/// [`crate::handshake`]'s job. Kept because it is genuinely useful for
/// *display*: a context window and a price help a human choose.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AdvertisedMetadata {
    pub context_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    /// The provider claims this is a reasoning model. Believed for display,
    /// verified by probe.
    pub reasoning: Option<bool>,
    /// Best-effort classification. An embedding model must never be offered
    /// as a judge, and several providers do list both in one call.
    pub is_embedding: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub advertised: AdvertisedMetadata,
}

/// The output shape being *requested*. Whether it was honoured is what the
/// handshake establishes — several providers accept this field and ignore it.
#[derive(Debug, Clone, PartialEq)]
pub enum Constraint {
    /// Exactly one of these strings. The cheapest useful constraint: one
    /// token out, and the answer needs no parsing.
    OneOf(Vec<String>),
    /// A JSON object matching this schema.
    Schema(serde_json::Value),
    None,
}

#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    /// The task. Trusted — written by OVIS, never by a document.
    pub instruction: String,
    /// Untrusted content. Separate field so it cannot be concatenated into
    /// the instruction by accident; [`crate::prompt`] is the only place the
    /// two are combined.
    pub document: Option<String>,
    pub constraint: Constraint,
    pub max_tokens: u32,
    /// Ask for a token distribution. Providers that cannot are not an error —
    /// the handshake records it and the judge degrades.
    pub want_logprobs: bool,
    /// Suppress a reasoning channel where the provider supports it. Without
    /// this a thinking model spends the whole token budget deliberating and
    /// returns an empty answer.
    pub suppress_thinking: bool,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>, instruction: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            instruction: instruction.into(),
            document: None,
            constraint: Constraint::None,
            max_tokens: 16,
            want_logprobs: false,
            suppress_thinking: true,
        }
    }

    pub fn with_document(mut self, document: impl Into<String>) -> Self {
        self.document = Some(document.into());
        self
    }

    pub fn constrained(mut self, constraint: Constraint) -> Self {
        self.constraint = constraint;
        self
    }

    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    pub fn logprobs(mut self, want: bool) -> Self {
        self.want_logprobs = want;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub text: String,
    /// `(token, probability)` at the first generated position, strongest
    /// first, when the provider returned them.
    pub logprobs: Option<Vec<(String, f64)>>,
    /// The response carried reasoning output — either in a dedicated field or
    /// as a channel sentinel in the text.
    pub had_thinking: bool,
    pub finish_reason: Option<String>,
    pub prompt_tokens: Option<u32>,
}

/// A configured endpoint.
pub struct Provider {
    pub kind: ProviderKind,
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl std::fmt::Debug for Provider {
    /// Hand-written so an API key cannot reach a log through a derived
    /// `Debug`, which is how secrets usually escape.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Provider {
    pub fn new(
        kind: ProviderKind,
        base_url: Option<&str>,
        api_key: Option<String>,
    ) -> CoreResult<Self> {
        let base_url = base_url
            .map(str::to_string)
            .or_else(|| kind.default_base_url().map(str::to_string))
            .ok_or_else(|| {
                CoreError::Invalid(format!(
                    "provider kind '{}' has no default endpoint; a base_url is required",
                    kind.code()
                ))
            })?;
        if kind.requires_key() && api_key.as_deref().unwrap_or("").is_empty() {
            return Err(CoreError::Invalid(format!(
                "provider kind '{}' requires an API key",
                kind.code()
            )));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            // Generous: a cold local model can take tens of seconds to load
            // before its first token.
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| CoreError::Invalid(format!("http client: {e}")))?;
        Ok(Self {
            kind,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn key(&self) -> CoreResult<&str> {
        self.api_key.as_deref().filter(|k| !k.is_empty()).ok_or_else(|| {
            CoreError::Invalid(format!("no API key configured for {}", self.kind.code()))
        })
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    /// Every model the endpoint offers, normalized.
    pub async fn list_models(&self) -> CoreResult<Vec<ModelInfo>> {
        let mut models = match self.kind {
            ProviderKind::OpenAiCompatible => openai::list_models(self).await?,
            ProviderKind::Gemini => gemini::list_models(self).await?,
            ProviderKind::Anthropic => anthropic::list_models(self).await?,
            ProviderKind::Ollama => ollama::list_models(self).await?,
            ProviderKind::LlamaCpp => llamacpp::list_models(self).await?,
        };
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models.dedup_by(|a, b| a.id == b.id);
        Ok(models)
    }

    /// One constrained completion.
    pub async fn complete(&self, req: &CompletionRequest) -> CoreResult<Completion> {
        match self.kind {
            ProviderKind::OpenAiCompatible => openai::complete(self, req).await,
            ProviderKind::Gemini => gemini::complete(self, req).await,
            ProviderKind::Anthropic => anthropic::complete(self, req).await,
            ProviderKind::Ollama => ollama::complete(self, req).await,
            ProviderKind::LlamaCpp => llamacpp::complete(self, req).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Send a request and decode JSON, keeping the response body out of the error
/// that reaches a client while preserving it for the log.
pub(crate) async fn send_json(
    req: reqwest::RequestBuilder,
    what: &str,
) -> CoreResult<serde_json::Value> {
    let response = req
        .send()
        .await
        .map_err(|e| CoreError::Invalid(format!("{what}: {e}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| CoreError::Invalid(format!("{what}: unreadable body: {e}")))?;
    if !status.is_success() {
        return Err(CoreError::Invalid(format!(
            "{what}: HTTP {status}: {}",
            truncate(&body, 400)
        )));
    }
    serde_json::from_str(&body)
        .map_err(|e| CoreError::Invalid(format!("{what}: malformed response: {e}")))
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

/// Sentinels that mean a model started a reasoning channel instead of
/// answering. Observed on the reference deployment's Gemma build, which opens
/// with a literal `<|channel>` token.
pub(crate) const THINKING_SENTINELS: [&str; 5] = [
    "<|channel>",
    "<|channel|>",
    "<think>",
    "<thinking>",
    "<|start_of_thinking|>",
];

pub(crate) fn looks_like_thinking(text: &str) -> bool {
    let head = text.trim_start();
    THINKING_SENTINELS.iter().any(|s| head.starts_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kinds_round_trip_and_reject_typos() {
        for kind in [
            ProviderKind::OpenAiCompatible,
            ProviderKind::Gemini,
            ProviderKind::Anthropic,
            ProviderKind::Ollama,
            ProviderKind::LlamaCpp,
        ] {
            assert_eq!(ProviderKind::parse(kind.code()).unwrap(), kind);
        }
        let err = ProviderKind::parse("openai").unwrap_err();
        assert!(err.to_string().contains("openai_compatible"), "{err}");
    }

    #[test]
    fn self_hosted_kinds_need_no_key_and_hosted_ones_do() {
        assert!(!ProviderKind::OpenAiCompatible.requires_key());
        assert!(!ProviderKind::LlamaCpp.requires_key());
        assert!(!ProviderKind::Ollama.requires_key());
        assert!(ProviderKind::Gemini.requires_key());
        assert!(ProviderKind::Anthropic.requires_key());
    }

    #[test]
    fn a_hosted_provider_without_a_key_is_refused_at_construction() {
        let err = Provider::new(ProviderKind::Gemini, None, None).unwrap_err();
        assert!(err.to_string().contains("requires an API key"), "{err}");
        assert!(Provider::new(
            ProviderKind::Gemini,
            None,
            Some("k".into())
        )
        .is_ok());
    }

    #[test]
    fn a_self_hosted_provider_without_a_base_url_is_refused() {
        let err = Provider::new(ProviderKind::OpenAiCompatible, None, None).unwrap_err();
        assert!(err.to_string().contains("base_url is required"), "{err}");
    }

    /// The most likely way a key escapes is a derived `Debug` in a log line.
    #[test]
    fn debug_never_prints_the_api_key() {
        let provider = Provider::new(
            ProviderKind::Gemini,
            None,
            Some("AIzaSy-super-secret-value".into()),
        )
        .unwrap();
        let rendered = format!("{provider:?}");
        assert!(!rendered.contains("super-secret"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn base_urls_normalize_trailing_slashes() {
        let p = Provider::new(
            ProviderKind::OpenAiCompatible,
            Some("http://host:8080/"),
            None,
        )
        .unwrap();
        assert_eq!(p.base_url(), "http://host:8080");
        assert_eq!(p.url("/v1/models"), "http://host:8080/v1/models");
        assert_eq!(p.url("v1/models"), "http://host:8080/v1/models");
    }

    #[test]
    fn thinking_sentinels_are_recognised_at_the_head_only() {
        assert!(looks_like_thinking("<|channel>final<|message>2"));
        assert!(looks_like_thinking("  <think>hmm"));
        assert!(!looks_like_thinking("2"));
        // A document that merely mentions the sentinel is not a thinking model.
        assert!(!looks_like_thinking("the answer is <think> in some models"));
    }
}
