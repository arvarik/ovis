//! The embedded UI.
//!
//! Hashed asset filenames are immutable, so they get a one-year cache;
//! `index.html` must not be cached at all, or a deploy leaves clients on the old
//! bundle pointing at a new API. The old handler set no cache headers of any
//! kind.

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../ui/dist"]
pub struct Assets;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache, must-revalidate";

/// Serve an embedded asset, falling back to `index.html` for client-side routes.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let target = if path.is_empty() { "index.html" } else { path };

    if let Some(asset) = Assets::get(target) {
        return respond(target, asset.data.into_owned());
    }

    // A missing *asset* is a 404; a missing *route* is the SPA's to handle.
    // Getting this backwards means a mistyped bundle URL silently returns HTML,
    // which surfaces as a bewildering JavaScript parse error.
    if is_asset_request(target) {
        return (StatusCode::NOT_FOUND, "404 Not Found").into_response();
    }

    match Assets::get("index.html") {
        Some(index) => respond("index.html", index.data.into_owned()),
        None => (
            StatusCode::NOT_FOUND,
            "UI assets are not embedded in this build",
        )
            .into_response(),
    }
}

fn respond(path: &str, body: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if is_immutable(path) { IMMUTABLE } else { NO_CACHE };

    match Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime.as_ref())
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        )
        .header(header::CACHE_CONTROL, HeaderValue::from_static(cache))
        .body(Body::from(body))
    {
        Ok(response) => response,
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "asset build failed").into_response(),
    }
}

/// Vite emits content-hashed files under `assets/`; those are safe to cache
/// forever. Everything else — `index.html`, `favicon.ico`, `manifest.json` — is
/// requested by a stable name and must revalidate.
fn is_immutable(path: &str) -> bool {
    path.starts_with("assets/")
}

/// Extensions that mean "this is a static file, 404 if absent".
///
/// An allow-list rather than "the last segment contains a dot", because document
/// ids are URLs: the UI route `/pages/https%3A%2F%2Fexample.com%2Fa` has a dotted
/// last segment and must still get the SPA shell.
const ASSET_EXTENSIONS: [&str; 18] = [
    "js", "mjs", "css", "map", "json", "txt", "xml", "ico", "png", "jpg", "jpeg", "gif", "svg",
    "webp", "avif", "woff", "woff2", "ttf",
];

/// Whether a miss should be a 404 rather than the SPA shell.
fn is_asset_request(path: &str) -> bool {
    if path.starts_with("assets/") {
        return true;
    }
    let Some(segment) = path.rsplit('/').next() else {
        return false;
    };
    let Some((_, extension)) = segment.rsplit_once('.') else {
        return false;
    };
    ASSET_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_bundles_are_cached_forever_and_the_shell_is_not() {
        assert!(is_immutable("assets/index-a1b2c3d4.js"));
        assert!(is_immutable("assets/style-deadbeef.css"));
        assert!(!is_immutable("index.html"));
        assert!(!is_immutable("favicon.ico"));
        assert!(!is_immutable("manifest.json"));
    }

    #[test]
    fn a_missing_bundle_is_a_404_not_the_html_shell() {
        // Returning index.html for a .js request produces
        // "Uncaught SyntaxError: Unexpected token '<'", which is a miserable
        // thing to debug.
        assert!(is_asset_request("assets/index-deleted.js"));
        assert!(is_asset_request("favicon.ico"));
        assert!(is_asset_request("robots.txt"));
        assert!(is_asset_request("nested/path/thing.css"));
    }

    #[test]
    fn client_side_routes_fall_back_to_the_shell() {
        assert!(!is_asset_request("pages"));
        assert!(!is_asset_request("pages/detail"));
        assert!(!is_asset_request("connectors/42/attempts"));
        assert!(!is_asset_request(""));
        // A route that happens to end in .html still gets the shell.
        assert!(!is_asset_request("index.html"));
    }

    #[test]
    fn a_document_id_shaped_route_is_not_mistaken_for_an_asset() {
        // Document ids are URLs, so a UI route's last segment is full of dots.
        // These must all reach the SPA shell, not 404.
        for route in [
            "pages/https%3A%2F%2Fexample.com%2Fa",
            "pages/example.com",
            "pages/https%3A%2F%2Faswathdamodaran.blogspot.com%2F2017%2F10%2Ftax-reform",
            "connectors/42",
        ] {
            assert!(
                !is_asset_request(route),
                "{route} must fall back to the SPA shell"
            );
        }
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert!(is_asset_request("LOGO.PNG"));
        assert!(is_asset_request("bundle.JS"));
    }
}
