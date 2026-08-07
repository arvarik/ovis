//! Provider-agnostic LLM access: model discovery, capability probing, and
//! constrained scoring.
//!
//! This crate exists apart from `ovis-core` for two reasons. It is the only
//! part of OVIS that makes network calls to a third party at a URL the user
//! supplied, and it is the only part whose correctness depends on what a
//! remote model *actually does* rather than on what an API says it will do.
//! Keeping it separate also means pruning never gains a hard dependency on an
//! LLM being configured — every existing detector keeps working with no
//! provider at all.
//!
//! # The load-bearing idea: probe, do not trust
//!
//! Every metadata source we checked was wrong about something on the reference
//! deployment:
//!
//! * `structured_output` is absent on 38% of the 6,106 models in the
//!   models.dev catalogue.
//! * Two independent research passes over Google's own documentation and its
//!   SDK reached *opposite* conclusions about whether `text/x.enum` still
//!   exists. A thirty-second probe settled it — it does.
//! * Gemini documents logprobs as generally available. No model on the
//!   reference key has them.
//! * llama.cpp's `/v1/models` reports a filename and `capabilities:
//!   ["completion"]`, and nothing about grammars, context, or the fact that
//!   the served model emits a reasoning channel that swallows the answer.
//!
//! Worse, four documented provider behaviours return **200 OK with plausible
//! output** while silently ignoring the constraint they were given. So
//! [`handshake`] verifies that a constraint was *enforced*, not that a request
//! was *accepted*, and a model that fails cannot be assigned a role.
//!
//! # Injection posture
//!
//! Document text from a web crawl is untrusted input. [`CompletionRequest`]
//! keeps the instruction and the document in separate fields so a caller
//! cannot concatenate them by accident, and [`prompt`] is the only place the
//! two are combined — always with the document delimited and declared as data.

pub mod handshake;
pub mod judge;
pub mod narrate;
pub mod prompt;
pub mod provider;

pub use handshake::{Capabilities, ThinkingChannel};
pub use judge::{Grade, Judge};
pub use narrate::{Narration, Narrator};
pub use provider::{Completion, CompletionRequest, Constraint, ModelInfo, Provider, ProviderKind};
