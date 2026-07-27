//! OVIS core — the data plane.
//!
//! Everything that talks to an external system lives here, behind typed
//! functions:
//!
//! * [`db`] — SQL against the Onyx Postgres database (read-only except for
//!   per-document delete/edit, which Onyx has no API for).
//! * [`search`] — the OpenSearch chunk index and the vLLM embedding endpoint.
//! * [`onyx`] — the Onyx HTTP API, used for every connector/indexing action.
//!
//! Route handlers in `ovis-backend` carry no SQL and no OpenSearch JSON; they
//! parse, call one function from here, and map the result. [`api_types`] holds
//! the wire structs shared with the CLI so wire compatibility is
//! compiler-checked rather than hand-maintained.

pub mod api_types;
pub mod cursor;
pub mod db;
pub mod error;
pub mod onyx;
pub mod search;

pub use error::{CoreError, CoreResult};

/// Sentinel `index_attempt.error_msg` substrings written by the homelab
/// resilience cron (`onyx_resilience_cron.sh`). A cc-pair whose most recent
/// attempt carries one of these is deliberately *parked*: the cron skips it and
/// OVIS must never clobber the message or silently re-trigger a crawl. Both
/// UIs surface it as a `parked` badge and require an explicit acknowledgement
/// before `run-once`.
pub const PARKED_SENTINELS: [&str; 2] = ["first-pass already complete", "park done"];

/// True when an `index_attempt.error_msg` marks its cc-pair as parked.
pub fn is_parked_error(error_msg: Option<&str>) -> bool {
    match error_msg {
        Some(msg) => PARKED_SENTINELS.iter().any(|s| msg.contains(s)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parked_detection_matches_both_sentinels_and_nothing_else() {
        assert!(is_parked_error(Some("first-pass already complete")));
        assert!(is_parked_error(Some("park done")));
        assert!(is_parked_error(Some(
            "skipping: first-pass already complete for cc_pair 42"
        )));
        assert!(!is_parked_error(Some("connection reset by peer")));
        assert!(!is_parked_error(Some("")));
        assert!(!is_parked_error(None));
    }
}
