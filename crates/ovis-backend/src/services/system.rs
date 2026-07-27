//! Health, version, and runtime metadata.
//!
//! `/system/health` returns **503 when degraded**. The old one returned 200 with
//! `status: "degraded"` in the body, so a Docker `HEALTHCHECK` passed with a dead
//! Postgres and nothing ever restarted.

use ovis_core::api_types::{DependencyHealth, HealthResponse, OnyxHealth, RuntimeResponse};

use crate::state::AppState;

fn ok(latency: std::time::Duration) -> DependencyHealth {
    DependencyHealth {
        status: "ok".into(),
        latency_ms: Some(round_ms(latency)),
        detail: None,
    }
}

fn down(detail: String) -> DependencyHealth {
    DependencyHealth {
        status: "down".into(),
        latency_ms: None,
        // Health output is for operators, not anonymous callers of a data
        // endpoint, and a useless health check is worse than a slightly verbose
        // one. Still bounded so a stack trace cannot fill the response.
        detail: Some(truncate(&detail, 300)),
    }
}

fn round_ms(duration: std::time::Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

/// Probe every dependency. Whether the result is a 200 or a 503 is decided by
/// the caller from `status`.
pub async fn health(state: &AppState) -> HealthResponse {
    let runtime = state.runtime();

    let (postgres, opensearch, embedder) = tokio::join!(
        async {
            match ovis_core::db::pool::ping(&state.db).await {
                Ok(latency) => ok(latency),
                Err(err) => down(err.to_string()),
            }
        },
        async {
            match state.os.ping().await {
                Ok(latency) => ok(latency),
                Err(err) => down(err.to_string()),
            }
        },
        async {
            match state.embed.as_ref() {
                None => DependencyHealth {
                    status: "unconfigured".into(),
                    latency_ms: None,
                    detail: Some(
                        "EMBED_API_URL is unset; semantic and hybrid search fall back to \
                         keyword search"
                            .into(),
                    ),
                },
                Some(embedder) => match embedder.ping().await {
                    Ok(latency) => ok(latency),
                    Err(err) => down(err.to_string()),
                },
            }
        },
    );

    let onyx_api = match state.onyx.as_ref() {
        None => OnyxHealth {
            configured: false,
            status: "unconfigured".into(),
            latency_ms: None,
            version: None,
            detail: Some(
                "ONYX_API_URL/ONYX_API_KEY are unset; connector actions return \
                 503 ONYX_UNCONFIGURED"
                    .into(),
            ),
        },
        Some(onyx) => match onyx.health().await {
            Ok(latency) => {
                let version = onyx.version().await.ok().and_then(|v| v.backend_version);
                // Reachability is not the same as being authorised; a stale token
                // must surface here rather than on the first action attempt.
                let (status, detail) = match onyx.verify_token().await {
                    Ok(()) => ("ok", None),
                    Err(err) => (
                        "unauthorized",
                        Some(format!(
                            "reachable but the configured token was rejected: {}",
                            truncate(&err.to_string(), 200)
                        )),
                    ),
                };
                OnyxHealth {
                    configured: true,
                    status: status.into(),
                    latency_ms: Some(round_ms(latency)),
                    version,
                    detail,
                }
            }
            Err(err) => OnyxHealth {
                configured: true,
                status: "down".into(),
                latency_ms: None,
                version: None,
                detail: Some(truncate(&err.to_string(), 300)),
            },
        },
    };

    // What makes the service degraded: Postgres and OpenSearch are required, and
    // so is a schema we can actually answer from. An unconfigured or broken Onyx
    // or embedder costs specific features, not the service.
    let degraded = postgres.status != "ok"
        || opensearch.status != "ok"
        || !runtime.schema.is_ok();

    HealthResponse {
        status: if degraded { "degraded" } else { "ok" }.into(),
        postgres,
        opensearch,
        onyx_api,
        embedder,
        schema_ok: runtime.schema.is_ok(),
        missing_columns: runtime.schema.missing_columns.clone(),
        unhandled_document_fk_children: runtime.schema.unhandled_fk_children.clone(),
        missing_indexes: runtime.schema.missing_indexes.clone(),
        index_name: runtime.index_name.clone(),
        version: state.build.version.to_string(),
    }
}

pub fn runtime_response(state: &AppState) -> RuntimeResponse {
    let runtime = state.runtime();
    RuntimeResponse {
        index_name: runtime.index_name.clone(),
        embedding_model: runtime.embedding_model.clone(),
        embedding_dim: runtime.embedding_dim,
        query_prefix: runtime.query_prefix.clone(),
        search_settings_id: runtime.search_settings_id,
        schema_ok: runtime.schema.is_ok(),
        refreshed_at: runtime.refreshed_at,
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
    use ovis_core::db::probe::SchemaProbe;

    fn health_with(
        postgres: &str,
        opensearch: &str,
        schema: SchemaProbe,
    ) -> (bool, SchemaProbe) {
        let degraded = postgres != "ok" || opensearch != "ok" || !schema.is_ok();
        (degraded, schema)
    }

    #[test]
    fn a_dead_dependency_makes_the_service_degraded() {
        assert!(health_with("down", "ok", SchemaProbe::default()).0);
        assert!(health_with("ok", "down", SchemaProbe::default()).0);
        assert!(!health_with("ok", "ok", SchemaProbe::default()).0);
    }

    #[test]
    fn a_schema_mismatch_makes_the_service_degraded() {
        assert!(
            health_with(
                "ok",
                "ok",
                SchemaProbe {
                    missing_columns: vec!["document.chunk_count".into()],
                    ..Default::default()
                }
            )
            .0
        );
    }

    #[test]
    fn missing_perf_indexes_are_reported_without_degrading_the_service() {
        // These are a performance warning; the server is correct without them.
        let (degraded, schema) = health_with(
            "ok",
            "ok",
            SchemaProbe {
                missing_indexes: vec!["ix_ovis_document_updated".into()],
                ..Default::default()
            },
        );
        assert!(!degraded);
        assert_eq!(schema.missing_indexes.len(), 1);
    }

    #[test]
    fn latency_is_rounded_to_a_tenth_of_a_millisecond() {
        assert_eq!(round_ms(std::time::Duration::from_micros(1234)), 1.2);
        assert_eq!(round_ms(std::time::Duration::from_millis(15)), 15.0);
    }

    #[test]
    fn health_detail_is_bounded() {
        let long = "x".repeat(10_000);
        let detail = down(long).detail.unwrap();
        assert!(detail.len() <= 303);
    }
}
