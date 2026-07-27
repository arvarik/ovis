//! Finish rendering handler errors, now that the request id is known.
//!
//! `IntoResponse for AppError` cannot see the request id — handlers return
//! `Result<_, AppError>` and axum converts them with no access to request
//! extensions. It therefore stashes the error in *response* extensions, and this
//! layer re-renders the body with the real id and emits the single log line that
//! carries the full internal detail.
//!
//! Without this, every error envelope would say `req_id: "-"`, which makes the
//! one field that ties a client-visible `"database error"` to the log line
//! holding the actual cause useless.

use axum::extract::Request;
use axum::response::Response;

use crate::error::{AppError, RequestId};

pub async fn render_errors(request: Request, next: axum::middleware::Next) -> Response {
    let req_id = request
        .extensions()
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| "-".to_string());

    let mut response = next.run(request).await;

    match response.extensions_mut().remove::<AppError>() {
        Some(err) => err.into_response_with_id(&req_id),
        None => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn the_deferred_error_is_rendered_with_the_request_id() {
        let handler_response =
            AppError::Database("password authentication failed".into()).into_response();
        // Before the layer runs, the body has the placeholder.
        let mut carried = handler_response;
        let err = carried
            .extensions_mut()
            .remove::<AppError>()
            .expect("carried");
        let rendered = err.into_response_with_id("ms2djpd70071");

        assert_eq!(
            rendered.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        let body = body_json(rendered).await;
        assert_eq!(body["error"]["req_id"], "ms2djpd70071");
        assert_eq!(body["error"]["code"], "DATABASE");
        assert_eq!(body["error"]["message"], "database error");
        assert_eq!(body["error"]["status"], 500);
        // The driver text stays in the log, not the response.
        assert!(!body.to_string().contains("password authentication"));
    }

    #[tokio::test]
    async fn a_successful_response_passes_through_untouched() {
        let mut response =
            (axum::http::StatusCode::OK, Body::from("{\"ok\":true}")).into_response();
        assert!(response.extensions_mut().remove::<AppError>().is_none());
    }

    #[tokio::test]
    async fn every_error_variant_survives_the_extension_round_trip() {
        for err in [
            AppError::NotFound {
                what: "document",
                id: "x".into(),
            },
            AppError::BadRequest("nope".into()),
            AppError::Unauthorized,
            AppError::Conflict("busy".into()),
            AppError::ParkedConnector("ack".into()),
            AppError::UpstreamOnyx {
                status: 500,
                body: "trace".into(),
            },
            AppError::UpstreamSearch("boom".into()),
            AppError::UpstreamEmbed("boom".into()),
            AppError::Database("boom".into()),
            AppError::Timeout("the request"),
            AppError::OnyxUnconfigured,
            AppError::SchemaMismatch("column".into()),
            AppError::NotAvailable("no vectors".into()),
        ] {
            let expected_status = err.status();
            let expected_code = err.code();
            let mut response = err.into_response();
            let carried = response
                .extensions_mut()
                .remove::<AppError>()
                .expect("error must be carried for re-rendering");
            assert_eq!(carried.status(), expected_status);
            assert_eq!(carried.code(), expected_code);
            let body = body_json(carried.into_response_with_id("REQ")).await;
            assert_eq!(body["error"]["req_id"], "REQ");
            assert_eq!(body["error"]["code"], expected_code);
        }
    }
}
