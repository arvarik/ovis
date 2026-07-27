//! Router assembly for `/api/v1`.

use axum::response::IntoResponse;
use axum::routing::{delete, get, patch, post};
use axum::Router;

use crate::state::AppState;

pub mod connectors;
pub mod indexing;
pub mod pages;
pub mod prune;
pub mod search;
pub mod stats;
pub mod stream;
pub mod system;
pub mod tags;

/// Routes that opt out of the global request timeout because they are
/// long-lived by design.
pub const SSE_ROUTES: [&str; 1] = ["/api/v1/pages/stream"];

/// Build the `/api/v1` router.
///
/// **Document ids occupy exactly one path segment and must be percent-encoded**
/// (`https%3A%2F%2Fexample.com%2Fa`), as the API contract requires. A terminal
/// `{*id}` catch-all would accept them raw, but axum 0.8 only permits a catch-all
/// as the *last* segment of a route — which rules out `/pages/{*id}/chunks`, and
/// having detail accept raw ids while its sub-resources did not would be worse
/// than requiring encoding consistently.
///
/// An unencoded id therefore matches no route; [`api_fallback`] answers those
/// with a 404 that says so, rather than letting the request fall through to the
/// SPA and return HTML to a JSON client.
///
/// Route order matters: the literal `/pages/batch-delete` is declared before
/// `/pages/{id}`, or it would be captured as a document id.
///
/// `/pages/stream` is deliberately absent here: `app()` mounts it on a router
/// without the request-timeout layer, because a stream is long-lived by design.
pub fn api_router(state: AppState) -> Router {
    Router::new()
        // --- system ---
        .route("/system/health", get(system::health))
        .route("/system/version", get(system::version))
        .route("/system/runtime", get(system::runtime))
        .route("/system/metrics", get(system::metrics))
        // --- pages ---
        .route("/pages", get(pages::list))
        .route("/pages/batch-delete", post(pages::batch_delete))
        .route("/pages/{id}/chunks", get(pages::chunks))
        .route(
            "/pages/{id}/chunks/{chunk_index}/vector",
            get(pages::chunk_vector),
        )
        .route("/pages/{id}/text", get(pages::text))
        .route(
            "/pages/{id}",
            get(pages::detail).patch(pages::patch).delete(pages::delete),
        )
        // --- search ---
        .route("/search", get(search::search))
        // --- connectors ---
        .route("/connectors", get(connectors::list))
        .route("/connectors/{cc_pair_id}", get(connectors::detail))
        .route(
            "/connectors/{cc_pair_id}/attempts",
            get(connectors::attempts),
        )
        .route("/connectors/{cc_pair_id}/errors", get(connectors::errors))
        .route("/connectors/{cc_pair_id}/docs", get(connectors::docs))
        .route("/connectors/{cc_pair_id}/pause", post(connectors::pause))
        .route("/connectors/{cc_pair_id}/resume", post(connectors::resume))
        .route(
            "/connectors/{cc_pair_id}/run-once",
            post(connectors::run_once),
        )
        .route("/connectors/{cc_pair_id}/prune", post(connectors::prune))
        .route("/connectors/{cc_pair_id}", patch(connectors::patch))
        .route("/connectors/{cc_pair_id}", delete(connectors::delete))
        // --- indexing ---
        .route("/indexing/attempts", get(indexing::attempts))
        .route("/indexing/attempts/{attempt_id}", get(indexing::attempt))
        .route(
            "/indexing/background-errors",
            get(indexing::background_errors),
        )
        .route(
            "/indexing/failed-documents",
            get(indexing::failed_documents),
        )
        .route(
            "/indexing/targeted-reindex",
            post(indexing::targeted_reindex),
        )
        .route(
            "/indexing/targeted-reindex/{job_id}",
            get(indexing::targeted_reindex_status),
        )
        // --- prune ---
        // Literal segments are declared before `{id}` captures; axum gives
        // literals precedence, so `stage` can never be parsed as an id.
        .route("/prune/status", get(prune::status))
        .route("/prune/candidates", get(prune::candidates))
        .route("/prune/candidates/stage", post(prune::stage))
        .route("/prune/candidates/dismiss", post(prune::dismiss))
        .route("/prune/candidates/restore", post(prune::restore))
        .route(
            "/prune/candidates/schedule-delete",
            post(prune::schedule_delete),
        )
        .route("/prune/candidates/{id}", get(prune::candidate_detail))
        .route(
            "/prune/scans",
            get(prune::list_scans).post(prune::create_scan),
        )
        .route("/prune/scans/{id}", get(prune::scan_detail))
        .route("/prune/scans/{id}/cancel", post(prune::cancel_scan))
        .route(
            "/prune/rules",
            get(prune::list_rules).post(prune::create_rule),
        )
        .route(
            "/prune/rules/{id}",
            patch(prune::patch_rule).delete(prune::delete_rule),
        )
        .route("/prune/rules/{id}/preview", post(prune::preview_rule))
        .route(
            "/prune/config",
            get(prune::export_config).put(prune::import_config),
        )
        .route("/prune/audit", get(prune::audit))
        .route("/prune/exclusions", get(prune::exclusions))
        .route("/prune/exclusions/{id}", delete(prune::delete_exclusion))
        // --- tags ---
        .route("/tags", get(tags::list))
        .route("/tags/keys", get(tags::keys))
        // --- stats ---
        .route("/stats/overview", get(stats::overview))
        .route("/stats/index", get(stats::index))
        .route("/stats/sources", get(stats::sources))
        .route("/stats/connectors/top", get(stats::top_connectors))
        .route("/stats/timeline", get(stats::timeline))
        .fallback(api_fallback)
        .with_state(state)
}

/// Answer an unmatched `/api/v1/*` path with JSON.
///
/// Without this, an unknown API path falls through to the SPA handler and a JSON
/// client gets `200 OK` with an HTML document — which surfaces as a parse error
/// somewhere far away from the actual mistake. The commonest cause is a document
/// id that was not percent-encoded, so the message says so.
async fn api_fallback(uri: axum::http::Uri) -> axum::response::Response {
    let path = uri.path();

    // A raw `https://…` in the path is a client mistake with a specific fix, so
    // say what it is rather than shrugging with a bare 404.
    if path.contains("://") || path.matches('/').count() > 3 {
        return crate::error::AppError::BadRequest(format!(
            "no route matches '{path}'. Document ids occupy exactly one path segment and must \
             be percent-encoded, e.g. /api/v1/pages/https%3A%2F%2Fexample.com%2Fa"
        ))
        .into_response();
    }

    crate::error::AppError::NotFound {
        what: "route",
        id: path.to_string(),
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn sse_routes_are_declared_for_timeout_exemption() {
        // The global 30 s timeout would otherwise kill a stream mid-flight.
        assert!(super::SSE_ROUTES.contains(&"/api/v1/pages/stream"));
    }
}
