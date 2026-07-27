//! Extractors that fail loudly.
//!
//! Axum's built-in `Query`/`Json` rejections produce a bare text body with no
//! error code. These wrappers reuse the standard extractors and map their
//! rejections into the OVIS error envelope, so a client sees
//! `{"error": {"code": "BAD_REQUEST", …}}` for a typo'd parameter exactly as it
//! does for every other 400.
//!
//! The query types themselves carry `#[serde(deny_unknown_fields)]`, which is
//! what turns `?sortt=updated_desc` into a 400 instead of a silently ignored
//! parameter and a confusingly ordered page.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::response::Response;

use crate::error::{AppError, RequestId};

fn req_id_from_parts(parts: &Parts) -> String {
    parts
        .extensions
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| "-".to_string())
}

/// `Query<T>` with an OVIS-shaped rejection.
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Query(value)),
            Err(rejection) => {
                let req_id = req_id_from_parts(parts);
                Err(AppError::BadRequest(describe_query_rejection(&rejection))
                    .into_response_with_id(&req_id))
            }
        }
    }
}

/// `Json<T>` with an OVIS-shaped rejection.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let req_id = request
            .extensions()
            .get::<RequestId>()
            .map(|r| r.0.clone())
            .unwrap_or_else(|| "-".to_string());

        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Json(value)),
            Err(rejection) => Err(AppError::BadRequest(describe_json_rejection(&rejection))
                .into_response_with_id(&req_id)),
        }
    }
}

fn describe_query_rejection(rejection: &QueryRejection) -> String {
    let detail = rejection.body_text();
    if detail.contains("unknown field") {
        format!("{detail}. Query parameters are validated strictly; check for a typo.")
    } else {
        detail
    }
}

fn describe_json_rejection(rejection: &JsonRejection) -> String {
    match rejection {
        JsonRejection::JsonDataError(e) => e.body_text(),
        JsonRejection::JsonSyntaxError(e) => e.body_text(),
        JsonRejection::MissingJsonContentType(_) => {
            "expected a request body with Content-Type: application/json".into()
        }
        other => other.body_text(),
    }
}

/// Percent-decode a wildcard-captured document id.
///
/// Document ids are URLs, so clients percent-encode them into the path. Axum
/// hands back the raw segment; this turns `https%3A%2F%2Fa%2Fb` back into
/// `https://a/b`. An already-decoded id passes through unchanged, which keeps
/// hand-written `curl` calls working.
pub fn decode_path_id(raw: &str) -> String {
    let trimmed = raw.trim_start_matches('/');
    percent_encoding::percent_decode_str(trimmed)
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_document_ids_round_trip() {
        assert_eq!(
            decode_path_id("https%3A%2F%2Fexample.com%2Fa%3Fb%3D1"),
            "https://example.com/a?b=1"
        );
    }

    #[test]
    fn an_unencoded_id_passes_through() {
        // axum's `{*id}` wildcard captures the rest of the path verbatim, so a
        // hand-typed URL arrives already readable.
        assert_eq!(
            decode_path_id("https://example.com/a"),
            "https://example.com/a"
        );
        assert_eq!(
            decode_path_id("/https://example.com/a"),
            "https://example.com/a"
        );
    }

    #[test]
    fn ids_with_spaces_and_unicode_decode() {
        assert_eq!(decode_path_id("a%20b"), "a b");
        assert_eq!(decode_path_id("caf%C3%A9"), "café");
    }

    #[test]
    fn invalid_percent_escapes_do_not_panic() {
        // Malformed input must degrade, not abort the request handler.
        assert_eq!(decode_path_id("%zz"), "%zz");
        assert_eq!(decode_path_id("%"), "%");
    }
}
