//! Request timeout.
//!
//! Hand-rolled rather than `tower_http::timeout`, which answers with a bare
//! status and no body: every non-2xx from this API is supposed to carry the same
//! `{"error": {...}}` envelope, including a `req_id` that ties it to the log.
//!
//! SSE routes are mounted outside this layer — they are long-lived by design.

use axum::extract::{Request, State};
use axum::response::Response;

use crate::error::{AppError, RequestId};
use crate::state::AppState;

pub async fn enforce_timeout(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let req_id = request
        .extensions()
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| "-".to_string());
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let budget = state.cfg.request_timeout();

    match tokio::time::timeout(budget, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::error!(
                req_id,
                %method,
                path,
                timeout_secs = budget.as_secs(),
                "request exceeded its time budget and was abandoned"
            );
            AppError::Timeout("the request").into_response_with_id(&req_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_is_a_504_with_the_standard_envelope() {
        let err = AppError::Timeout("the request");
        assert_eq!(err.status(), axum::http::StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(err.code(), "TIMEOUT");
        assert!(err.client_message().contains("timed out"));
    }
}
