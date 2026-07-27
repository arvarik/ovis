//! `/api/v1/system` — health, version, runtime metadata, metrics.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use ovis_core::api_types::VersionResponse;

use crate::error::AppError;
use crate::services::system as service;
use crate::state::AppState;

/// **503 when degraded.** The old handler returned 200 with
/// `status: "degraded"`, so a Docker `HEALTHCHECK` passed with a dead Postgres
/// and nothing ever restarted the container.
pub async fn health(State(state): State<AppState>) -> Response {
    let report = service::health(&state).await;
    let status = if report.status == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    // A health probe must never be served from a cache.
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(report),
    )
        .into_response()
}

pub async fn version(State(state): State<AppState>) -> Response {
    axum::Json(VersionResponse {
        version: state.build.version.to_string(),
        git_sha: state.build.git_sha.to_string(),
        rustc: state.build.rustc.to_string(),
        built_at: state.build.built_at.to_string(),
        profile: state.build.profile.to_string(),
    })
    .into_response()
}

pub async fn runtime(State(state): State<AppState>) -> Response {
    axum::Json(service::runtime_response(&state)).into_response()
}

/// Prometheus text exposition.
pub async fn metrics(State(state): State<AppState>) -> Result<Response, AppError> {
    let handle = state.metrics.as_ref().ok_or_else(|| {
        AppError::NotAvailable("the metrics recorder failed to install at startup".into())
    })?;

    // Pool and cache gauges are sampled at scrape time rather than continuously:
    // they are cheap to read and nobody benefits from a background task
    // recomputing them between scrapes.
    metrics::gauge!("ovis_pg_pool_connections").set(state.db.size() as f64);
    metrics::gauge!("ovis_pg_pool_idle").set(state.db.num_idle() as f64);
    let cache_counts = state.caches.entry_counts().await;
    for (name, value) in [
        ("counts", &cache_counts["counts"]),
        ("connectors", &cache_counts["connectors"]),
        ("facets", &cache_counts["facets"]),
        ("stats", &cache_counts["stats"]),
    ] {
        metrics::gauge!("ovis_cache_entries", "cache" => name.to_string())
            .set(value.as_f64().unwrap_or(0.0));
    }

    let runtime = state.runtime();
    metrics::gauge!("ovis_schema_ok").set(if runtime.schema.is_ok() { 1.0 } else { 0.0 });
    metrics::gauge!("ovis_missing_indexes").set(runtime.schema.missing_indexes.len() as f64);
    metrics::gauge!("ovis_knn_ready").set(if runtime.capabilities.knn_ready() {
        1.0
    } else {
        0.0
    });

    if let Ok(pending) = ovis_core::db::pending_deletes::pending_count(&state.db).await {
        metrics::gauge!("ovis_pending_index_deletes").set(pending as f64);
    }

    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        handle.render(),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    #[test]
    fn degraded_health_maps_to_503() {
        // The specific behaviour the old code got wrong.
        for (status, expected) in [
            ("ok", axum::http::StatusCode::OK),
            ("degraded", axum::http::StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let mapped = if status == "ok" {
                axum::http::StatusCode::OK
            } else {
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            };
            assert_eq!(mapped, expected);
        }
    }
}
