//! One error type for the whole data plane.
//!
//! The point of this enum is that a given *failure class* always arrives at the
//! HTTP layer as the same variant, so it can always map to the same status
//! code. The old code funnelled everything through `anyhow` and then guessed,
//! which is how the same dead-Postgres condition produced a 200, a 502 and a
//! 404 on three different routes.
//!
//! `Display` here is deliberately *detailed* — it is what gets logged. The
//! sanitised, client-facing message is produced by the HTTP layer, which never
//! echoes these strings for the `Db`/`Search`/`Embed`/`Onyx` variants.

use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    /// Postgres failed. Always a 500 — never an empty 200.
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    /// OpenSearch was unreachable, timed out, or returned a non-2xx.
    #[error("opensearch error: {0}")]
    Search(String),

    /// The embedding endpoint was unreachable or returned garbage. Callers that
    /// can degrade (hybrid/semantic search) catch this and fall back to BM25.
    #[error("embedding error: {0}")]
    Embed(String),

    /// The Onyx API returned a non-2xx, or could not be reached.
    #[error("onyx api error (status {status}): {body}")]
    Onyx { status: u16, body: String },

    /// `ONYX_API_URL`/`ONYX_API_KEY` are unset, so no action can be proxied.
    #[error("onyx api is not configured")]
    OnyxUnconfigured,

    #[error("{what} '{id}' not found")]
    NotFound { what: &'static str, id: String },

    /// The Onyx schema does not have something we depend on (a column moved, or
    /// a new FK child table appeared that our delete sweep does not cover).
    /// Surfaces as 501 rather than a mid-transaction explosion.
    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    /// Caller-supplied input was rejected by the data layer (bad cursor, page
    /// too deep, batch too large).
    #[error("invalid input: {0}")]
    Invalid(String),

    /// A guarded mutation was refused because of current state (e.g. run-once
    /// on a parked connector without acknowledgement).
    #[error("conflict: {0}")]
    Conflict(String),
}

impl CoreError {
    pub fn search(msg: impl std::fmt::Display) -> Self {
        CoreError::Search(msg.to_string())
    }

    pub fn embed(msg: impl std::fmt::Display) -> Self {
        CoreError::Embed(msg.to_string())
    }

    pub fn invalid(msg: impl std::fmt::Display) -> Self {
        CoreError::Invalid(msg.to_string())
    }

    pub fn not_found(what: &'static str, id: impl Into<String>) -> Self {
        CoreError::NotFound {
            what,
            id: id.into(),
        }
    }

    /// Stable machine-readable code, mirrored in the HTTP error envelope.
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::Db(_) => "DATABASE",
            CoreError::Search(_) => "OPENSEARCH_UPSTREAM",
            CoreError::Embed(_) => "EMBED_UPSTREAM",
            CoreError::Onyx { .. } => "ONYX_UPSTREAM",
            CoreError::OnyxUnconfigured => "ONYX_UNCONFIGURED",
            CoreError::NotFound { .. } => "NOT_FOUND",
            CoreError::SchemaMismatch(_) => "SCHEMA_MISMATCH",
            CoreError::Invalid(_) => "BAD_REQUEST",
            CoreError::Conflict(_) => "CONFLICT",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_per_failure_class() {
        assert_eq!(CoreError::Db(sqlx::Error::PoolClosed).code(), "DATABASE");
        assert_eq!(CoreError::search("boom").code(), "OPENSEARCH_UPSTREAM");
        assert_eq!(CoreError::embed("boom").code(), "EMBED_UPSTREAM");
        assert_eq!(
            CoreError::Onyx {
                status: 500,
                body: "x".into()
            }
            .code(),
            "ONYX_UPSTREAM"
        );
        assert_eq!(CoreError::OnyxUnconfigured.code(), "ONYX_UNCONFIGURED");
        assert_eq!(CoreError::not_found("document", "a").code(), "NOT_FOUND");
    }

    #[test]
    fn database_display_keeps_detail_for_logs() {
        // The HTTP layer must not forward this string, but the log must have it.
        let e = CoreError::Db(sqlx::Error::PoolTimedOut);
        assert!(e.to_string().contains("database error"));
        assert!(e.to_string().len() > "database error".len());
    }
}
