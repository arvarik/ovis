//! Request ids.
//!
//! Every log line and every error envelope carries the same `req_id`, which is
//! what makes "the client got a 500 saying `database error`" a one-grep
//! investigation rather than a guess.
//!
//! An inbound `x-request-id` is honoured (so a reverse proxy's id wins), and
//! otherwise a fresh sortable id is minted.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use tower_http::request_id::{MakeRequestId, RequestId};

use crate::error::RequestId as RequestIdExt;

/// Monotonic, time-ordered request ids without pulling in a uuid/ulid crate:
/// milliseconds since the epoch in base36, plus a per-process counter.
#[derive(Clone, Default)]
pub struct MakeRequestUlid {
    counter: std::sync::Arc<AtomicU64>,
}

impl MakeRequestUlid {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_id(&self) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let seq = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{}{:04}", base36(millis), seq % 10_000)
    }
}

impl MakeRequestId for MakeRequestUlid {
    fn make_request_id<B>(&mut self, _request: &axum::http::Request<B>) -> Option<RequestId> {
        let id = self.next_id();
        id.parse().ok().map(RequestId::new)
    }
}

fn base36(mut n: u64) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(ALPHABET[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Copy the request id from the header into request extensions, where handlers
/// and the error renderer can reach it, and echo it on the response.
pub async fn propagate_request_id(mut request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(sanitize)
        .unwrap_or_else(|| "-".to_string());

    request.extensions_mut().insert(RequestIdExt(id.clone()));

    let mut response = next.run(request).await;
    if let Ok(value) = axum::http::HeaderValue::from_str(&id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

/// Request ids reach logs and response headers, so an inbound one is bounded and
/// stripped of anything that could forge a log line or split a header.
fn sanitize(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "-".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_sort_in_creation_order() {
        let maker = MakeRequestUlid::new();
        let ids: Vec<String> = (0..1000).map(|_| maker.next_id()).collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "request ids must not collide");
        // Same millisecond, so ordering comes from the counter suffix.
        assert!(ids[0] < ids[999]);
    }

    #[test]
    fn ids_are_header_safe() {
        let maker = MakeRequestUlid::new();
        for _ in 0..50 {
            let id = maker.next_id();
            assert!(axum::http::HeaderValue::from_str(&id).is_ok());
            assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn inbound_ids_are_sanitized_against_header_and_log_injection() {
        assert_eq!(sanitize("abc-123_x.y"), "abc-123_x.y");
        assert_eq!(sanitize("bad\r\nX-Admin: true"), "badX-Admintrue");
        assert_eq!(sanitize(""), "-");
        assert_eq!(sanitize("   "), "-");
        assert_eq!(sanitize(&"a".repeat(500)).len(), 64);
    }

    #[test]
    fn base36_encoding_is_monotonic() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        // Equal-length encodings compare correctly as strings, which is what the
        // sortability claim rests on.
        assert!(base36(1_700_000_000_000) < base36(1_800_000_000_000));
    }
}
