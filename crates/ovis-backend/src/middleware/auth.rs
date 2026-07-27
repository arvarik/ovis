//! Optional bearer-token auth.
//!
//! Off by default, because the deployment target is a trusted LAN and the UI is
//! served from the same origin. Once `OVIS_API_TOKEN` is set — which the plan
//! calls for as soon as the server is fronted by Caddy on a routable hostname —
//! every `/api/v1/*` route requires it, with two deliberate carve-outs:
//!
//! * `/api/v1/system/health` stays open so Docker and Caddy health checks work
//!   without embedding a credential in the compose file.
//! * SSE accepts `?token=` as well as the header, because `EventSource` cannot
//!   set request headers.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::error::{AppError, RequestId};
use crate::state::AppState;

/// Paths reachable without a token even when auth is on.
const OPEN_PATHS: [&str; 1] = ["/api/v1/system/health"];

pub async fn require_bearer(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    let Some(expected) = state.cfg.api_token.as_deref() else {
        return Ok(next.run(request).await);
    };

    let path = request.uri().path();
    if OPEN_PATHS.contains(&path) {
        return Ok(next.run(request).await);
    }

    let presented = bearer_from_header(&request).or_else(|| token_from_query(&request));

    let authorized = presented
        .as_deref()
        .map(|token| constant_time_eq(token.as_bytes(), expected.as_bytes()))
        .unwrap_or(false);

    if authorized {
        Ok(next.run(request).await)
    } else {
        let req_id = request
            .extensions()
            .get::<RequestId>()
            .map(|r| r.0.clone())
            .unwrap_or_else(|| "-".to_string());
        tracing::warn!(
            req_id,
            path,
            presented = presented.is_some(),
            "rejected an unauthenticated request"
        );
        Err(AppError::Unauthorized.into_response_with_id(&req_id))
    }
}

fn bearer_from_header(request: &Request) -> Option<String> {
    let raw = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim().to_string())
}

/// `?token=` — only for `EventSource`, which cannot send headers.
fn token_from_query(request: &Request) -> Option<String> {
    let query = request.uri().query()?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token").then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .into_owned()
}

/// Compare in time independent of where the first difference falls, so a
/// caller cannot recover the token a byte at a time by timing responses.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Length is not secret (and leaking it via an early return would be no worse
    // than leaking it via the response), but the content comparison must not
    // short-circuit.
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn request_with(header: Option<&str>, uri: &str) -> Request {
        let mut builder = Request::builder().uri(uri);
        if let Some(value) = header {
            builder = builder.header(axum::http::header::AUTHORIZATION, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn bearer_parsing_is_scheme_insensitive_and_trims() {
        assert_eq!(
            bearer_from_header(&request_with(Some("Bearer abc123"), "/")),
            Some("abc123".to_string())
        );
        assert_eq!(
            bearer_from_header(&request_with(Some("bearer  abc123 "), "/")),
            Some("abc123".to_string())
        );
        assert_eq!(
            bearer_from_header(&request_with(Some("Basic abc123"), "/")),
            None
        );
        assert_eq!(bearer_from_header(&request_with(Some("abc123"), "/")), None);
        assert_eq!(bearer_from_header(&request_with(None, "/")), None);
    }

    #[test]
    fn sse_token_query_parameter_is_read_and_percent_decoded() {
        assert_eq!(
            token_from_query(&request_with(None, "/api/v1/pages/stream?token=abc123")),
            Some("abc123".to_string())
        );
        assert_eq!(
            token_from_query(&request_with(
                None,
                "/api/v1/pages/stream?limit=10&token=a%2Bb%3Dc&sort=id_asc"
            )),
            Some("a+b=c".to_string())
        );
        assert_eq!(
            token_from_query(&request_with(None, "/api/v1/pages/stream?limit=10")),
            None
        );
        // A parameter that merely contains "token" must not match.
        assert_eq!(
            token_from_query(&request_with(None, "/api/v1/pages?api_token=x")),
            None
        );
    }

    #[test]
    fn health_is_the_only_open_path() {
        assert!(OPEN_PATHS.contains(&"/api/v1/system/health"));
        for guarded in [
            "/api/v1/pages",
            "/api/v1/system/metrics",
            "/api/v1/system/runtime",
            "/api/v1/connectors/1/pause",
        ] {
            assert!(
                !OPEN_PATHS.contains(&guarded),
                "{guarded} must require a token"
            );
        }
    }

    #[test]
    fn constant_time_comparison_is_still_correct() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreu"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
        // Differences in the first and last byte are both detected.
        assert!(!constant_time_eq(b"aaaa", b"baaa"));
        assert!(!constant_time_eq(b"aaaa", b"aaab"));
    }
}
