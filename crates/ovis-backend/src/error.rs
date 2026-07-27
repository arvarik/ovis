//! The HTTP error model.
//!
//! Three rules, each replacing a specific defect:
//!
//! 1. **A failure is never a 200.** The old list handler ended every fallible
//!    call with `.unwrap_or_default()`, so a dead Postgres produced
//!    `200 OK {"total": 0, "items": []}` — indistinguishable from an empty
//!    database.
//! 2. **One failure class, one status code.** The same dead-Postgres condition
//!    used to yield 200 on `/pages`, 502 on `/connectors` and 404 on
//!    `DELETE /pages/{id}`.
//! 3. **Internal detail goes to the log, not the client.** `Database(e) =>
//!    e.to_string()` shipped raw driver text — potentially including host and
//!    DSN fragments — to unauthenticated callers. Now every response carries a
//!    `req_id` that ties it to the full detail in the log.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ovis_core::CoreError;
use serde::Serialize;
use serde_json::json;

/// `Clone` so a handler error can ride in response extensions until the
/// middleware — which is the layer that knows the request id — renders it.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError {
    #[error("{what} '{id}' not found")]
    NotFound { what: &'static str, id: String },

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("conflict: {0}")]
    Conflict(String),

    /// A guarded action refused because the connector is parked.
    #[error("connector is parked: {0}")]
    ParkedConnector(String),

    #[error("upstream onyx error (status {status})")]
    UpstreamOnyx { status: u16, body: String },

    #[error("upstream opensearch error")]
    UpstreamSearch(String),

    #[error("upstream embedding error")]
    UpstreamEmbed(String),

    #[error("database error")]
    Database(String),

    #[error("timeout in {0}")]
    Timeout(&'static str),

    #[error("onyx api is not configured")]
    OnyxUnconfigured,

    /// The Onyx schema no longer matches what this endpoint needs.
    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    /// Requested a capability this deployment cannot serve (for example a
    /// chunk vector on an index that stores none).
    #[error("not available: {0}")]
    NotAvailable(String),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound { .. } => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Conflict(_) | AppError::ParkedConnector(_) => StatusCode::CONFLICT,
            AppError::UpstreamOnyx { .. }
            | AppError::UpstreamSearch(_)
            | AppError::UpstreamEmbed(_) => StatusCode::BAD_GATEWAY,
            AppError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
            AppError::OnyxUnconfigured => StatusCode::SERVICE_UNAVAILABLE,
            AppError::SchemaMismatch(_) => StatusCode::NOT_IMPLEMENTED,
            AppError::NotAvailable(_) => StatusCode::NOT_IMPLEMENTED,
        }
    }

    /// Stable, machine-readable code. Clients branch on this, never on the
    /// message.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NotFound { .. } => "NOT_FOUND",
            AppError::BadRequest(_) => "BAD_REQUEST",
            AppError::Unauthorized => "UNAUTHORIZED",
            AppError::Conflict(_) => "CONFLICT",
            AppError::ParkedConnector(_) => "PARKED_CONNECTOR",
            AppError::UpstreamOnyx { .. } => "ONYX_UPSTREAM",
            AppError::UpstreamSearch(_) => "OPENSEARCH_UPSTREAM",
            AppError::UpstreamEmbed(_) => "EMBED_UPSTREAM",
            AppError::Database(_) => "DATABASE",
            AppError::Timeout(_) => "TIMEOUT",
            AppError::OnyxUnconfigured => "ONYX_UNCONFIGURED",
            AppError::SchemaMismatch(_) => "SCHEMA_MISMATCH",
            AppError::NotAvailable(_) => "NOT_AVAILABLE",
        }
    }

    /// What the client is told.
    ///
    /// Caller-caused errors get a useful message — the caller can act on it.
    /// Upstream and database failures get a fixed string; their detail is in the
    /// log under the same `req_id`.
    pub fn client_message(&self) -> String {
        match self {
            AppError::NotFound { what, id } => {
                format!("{what} '{}' not found", truncate(id, 200))
            }
            AppError::BadRequest(msg) => msg.clone(),
            AppError::Unauthorized => {
                "missing or invalid bearer token; set Authorization: Bearer <token> \
                 (SSE clients may use ?token=)"
                    .into()
            }
            AppError::Conflict(msg) => msg.clone(),
            AppError::ParkedConnector(msg) => msg.clone(),
            AppError::UpstreamOnyx { status, .. } => {
                format!("the Onyx API rejected the request (upstream status {status})")
            }
            AppError::UpstreamSearch(_) => "search index unavailable".into(),
            AppError::UpstreamEmbed(_) => "embedding service unavailable".into(),
            AppError::Database(_) => "database error".into(),
            AppError::Timeout(what) => format!("timed out waiting for {what}"),
            AppError::OnyxUnconfigured => {
                "connector actions require ONYX_API_URL and ONYX_API_KEY to be configured".into()
            }
            AppError::SchemaMismatch(msg) => msg.clone(),
            AppError::NotAvailable(msg) => msg.clone(),
        }
    }

    /// Full detail, for the log only.
    pub fn log_detail(&self) -> String {
        match self {
            AppError::UpstreamOnyx { status, body } => format!("onyx status {status}: {body}"),
            AppError::UpstreamSearch(detail)
            | AppError::UpstreamEmbed(detail)
            | AppError::Database(detail) => detail.clone(),
            other => other.to_string(),
        }
    }

    pub fn is_server_side(&self) -> bool {
        self.status().is_server_error()
    }
}

impl From<CoreError> for AppError {
    fn from(err: CoreError) -> Self {
        match err {
            // Keep the driver text for the log; the response says only
            // "database error".
            CoreError::Db(e) => AppError::Database(e.to_string()),
            CoreError::Search(detail) => AppError::UpstreamSearch(detail),
            CoreError::Embed(detail) => AppError::UpstreamEmbed(detail),
            CoreError::Onyx { status, body } => AppError::UpstreamOnyx { status, body },
            CoreError::OnyxUnconfigured => AppError::OnyxUnconfigured,
            CoreError::NotFound { what, id } => AppError::NotFound { what, id },
            CoreError::SchemaMismatch(detail) => AppError::SchemaMismatch(detail),
            CoreError::Invalid(msg) => AppError::BadRequest(msg),
            CoreError::Conflict(msg) => AppError::Conflict(msg),
        }
    }
}

/// The wire envelope for every non-2xx response.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    pub status: u16,
    /// Correlates with the `req_id` on every log line for this request.
    pub req_id: String,
}

/// Request id, injected by the middleware into request extensions.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl AppError {
    /// Render with the request id attached. Used by the middleware-aware
    /// handlers; the bare `IntoResponse` path falls back to `"-"`.
    pub fn into_response_with_id(self, req_id: &str) -> Response {
        let status = self.status();
        let code = self.code();

        if self.is_server_side() {
            tracing::error!(req_id, code, detail = %self.log_detail(), "request failed");
        } else {
            tracing::debug!(req_id, code, detail = %self.log_detail(), "request rejected");
        }

        let body = Json(ErrorEnvelope {
            error: ErrorBody {
                code,
                message: self.client_message(),
                status: status.as_u16(),
                req_id: req_id.to_string(),
            },
        });
        (status, body).into_response()
    }

    /// The same envelope as a plain JSON value, for the SSE `error` event.
    pub fn as_event_payload(&self, req_id: &str) -> serde_json::Value {
        json!({
            "code": self.code(),
            "message": self.client_message(),
            "status": self.status().as_u16(),
            "req_id": req_id,
        })
    }
}

impl IntoResponse for AppError {
    /// Produce the right status immediately, but defer rendering the body.
    ///
    /// Handlers return `Result<_, AppError>` and axum converts through this
    /// impl, which has no access to the request id. Rather than let every error
    /// envelope carry `req_id: "-"` — making the one field that correlates a
    /// client-visible failure with its log line useless — the error travels in
    /// response extensions and [`crate::middleware::render_errors`] finishes the
    /// job with the id in hand.
    fn into_response(self) -> Response {
        let status = self.status();
        let mut response = (status, Json(ErrorEnvelope::from(&self))).into_response();
        response.extensions_mut().insert(self);
        response
    }
}

impl From<&AppError> for ErrorEnvelope {
    fn from(err: &AppError) -> Self {
        ErrorEnvelope {
            error: ErrorBody {
                code: err.code(),
                message: err.client_message(),
                status: err.status().as_u16(),
                req_id: "-".to_string(),
            },
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_failure_class_maps_to_one_status_everywhere() {
        // The whole point: a Postgres failure is a 500, on every route.
        assert_eq!(
            AppError::Database("connection refused".into()).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            AppError::from(ovis_core::CoreError::Db(sqlx::Error::PoolClosed)).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            AppError::UpstreamSearch("boom".into()).status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            AppError::OnyxUnconfigured.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            AppError::SchemaMismatch("document.chunk_count".into()).status(),
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            AppError::ParkedConnector("ack required".into()).status(),
            StatusCode::CONFLICT
        );
        assert_eq!(AppError::Timeout("postgres").status(), StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn database_errors_never_reach_the_client_verbatim() {
        let leaky = "connection to server at \"192.168.4.113\", port 5433 failed: \
                     password authentication failed for user \"postgres\"";
        let err = AppError::Database(leaky.into());
        assert_eq!(err.client_message(), "database error");
        assert!(!err.client_message().contains("192.168"));
        assert!(!err.client_message().contains("password"));
        // ...but the log keeps everything.
        assert!(err.log_detail().contains("192.168.4.113"));
    }

    #[test]
    fn onyx_upstream_bodies_are_summarised_for_the_client() {
        let err = AppError::UpstreamOnyx {
            status: 403,
            body: "Traceback (most recent call last): SECRET".into(),
        };
        assert!(!err.client_message().contains("SECRET"));
        assert!(err.client_message().contains("403"));
        assert!(err.log_detail().contains("SECRET"));
    }

    #[test]
    fn caller_errors_keep_their_actionable_message() {
        let err = AppError::BadRequest("unknown sort 'chunk_desc'".into());
        assert_eq!(err.client_message(), "unknown sort 'chunk_desc'");
        assert!(!err.is_server_side());
    }

    #[test]
    fn a_very_long_id_is_truncated_in_the_not_found_message() {
        let long_id = format!("https://example.com/{}", "a".repeat(5000));
        let err = AppError::NotFound {
            what: "document",
            id: long_id,
        };
        let message = err.client_message();
        assert!(message.len() < 300, "{message}");
        assert!(message.contains('…'), "{message}");
        assert!(message.ends_with("not found"));
    }

    #[test]
    fn every_core_error_variant_has_a_distinct_http_mapping() {
        use ovis_core::CoreError;
        let cases: Vec<(CoreError, StatusCode, &str)> = vec![
            (
                CoreError::Db(sqlx::Error::PoolTimedOut),
                StatusCode::INTERNAL_SERVER_ERROR,
                "DATABASE",
            ),
            (
                CoreError::Search("x".into()),
                StatusCode::BAD_GATEWAY,
                "OPENSEARCH_UPSTREAM",
            ),
            (
                CoreError::Embed("x".into()),
                StatusCode::BAD_GATEWAY,
                "EMBED_UPSTREAM",
            ),
            (
                CoreError::Onyx {
                    status: 500,
                    body: "x".into(),
                },
                StatusCode::BAD_GATEWAY,
                "ONYX_UPSTREAM",
            ),
            (
                CoreError::OnyxUnconfigured,
                StatusCode::SERVICE_UNAVAILABLE,
                "ONYX_UNCONFIGURED",
            ),
            (
                CoreError::not_found("document", "a"),
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
            ),
            (
                CoreError::SchemaMismatch("x".into()),
                StatusCode::NOT_IMPLEMENTED,
                "SCHEMA_MISMATCH",
            ),
            (
                CoreError::Invalid("x".into()),
                StatusCode::BAD_REQUEST,
                "BAD_REQUEST",
            ),
            (
                CoreError::Conflict("x".into()),
                StatusCode::CONFLICT,
                "CONFLICT",
            ),
        ];
        for (core, status, code) in cases {
            let app = AppError::from(core);
            assert_eq!(app.status(), status, "{code}");
            assert_eq!(app.code(), code);
        }
    }

    #[test]
    fn the_sse_error_payload_matches_the_http_envelope_fields() {
        let err = AppError::Database("detail".into());
        let payload = err.as_event_payload("01JABC");
        assert_eq!(payload["code"], "DATABASE");
        assert_eq!(payload["message"], "database error");
        assert_eq!(payload["status"], 500);
        assert_eq!(payload["req_id"], "01JABC");
    }
}
