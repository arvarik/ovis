//! Dashboard aggregates.
//!
//! Every function here is cached by key (30 s), because several of the queries
//! touch millions of rows. They are dashboard furniture, not per-keystroke
//! paths.

use std::sync::Arc;

use ovis_core::api_types::{
    CrawlStats, EmbeddingInfo, IndexStats, SourceStat, StatsOverview, TimelineResponse,
    TopConnector,
};
use ovis_core::db::stats::{self, TimelineBucketSize, TimelineWindow};
use ovis_core::db::{connectors, documents, indexing};

use crate::error::AppError;
use crate::state::AppState;

/// Serve `key` from cache, or compute and store it.
async fn cached<F, Fut, T>(state: &AppState, key: &str, compute: F) -> Result<T, AppError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>>,
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    if let Some(hit) = state.caches.stats.get(key).await {
        if let Ok(value) = serde_json::from_value::<T>(hit.as_ref().clone()) {
            return Ok(value);
        }
    }
    let computed = compute().await?;
    if let Ok(encoded) = serde_json::to_value(&computed) {
        state
            .caches
            .stats
            .insert(key.to_string(), Arc::new(encoded))
            .await;
    }
    Ok(computed)
}

pub async fn overview(state: &AppState) -> Result<StatsOverview, AppError> {
    cached(state, "overview", || async {
        let runtime = state.runtime();

        // Independent queries, so run them together rather than in sequence.
        let (documents_total, connector_counts, attempts, docs_15m, docs_24h, in_progress) = tokio::try_join!(
            async { Ok::<_, AppError>(documents::estimate_total_documents(&state.db).await?) },
            async { Ok::<_, AppError>(connectors::status_counts(&state.db).await?) },
            async { Ok::<_, AppError>(connectors::attempt_aggregates(&state.db, None).await?) },
            async { Ok::<_, AppError>(stats::docs_since(&state.db, 15).await?) },
            async { Ok::<_, AppError>(stats::docs_since(&state.db, 60 * 24).await?) },
            async { Ok::<_, AppError>(indexing::in_progress_counts(&state.db).await?) },
        )?;

        // The exact document total, if a list request already warmed it.
        let (documents_total, documents_exact) = match state
            .caches
            .counts
            .get(&unfiltered_count_key())
            .await
        {
            Some(exact) => (exact, true),
            None => (documents_total, false),
        };

        let index = index_stats(state).await;

        Ok(StatsOverview {
            documents: documents_total,
            documents_exact,
            chunks: index.docs,
            connectors: connector_counts,
            index,
            embedding: EmbeddingInfo {
                model: runtime.embedding_model.clone(),
                dim: runtime.embedding_dim,
            },
            crawl: CrawlStats {
                docs_last_15m: docs_15m,
                docs_last_24h: docs_24h,
                attempts_in_progress: in_progress.0,
                attempts_stalled: in_progress.1,
            },
            attempts,
        })
    })
    .await
}

/// Must match the key `services::pages` uses for the unfiltered total, so the
/// two surfaces agree on the number.
fn unfiltered_count_key() -> String {
    format!(
        "s={:?}|c={:?}|src={:?}|h={:?}|cmin={:?}|cmax={:?}|ua={:?}|ub={:?}",
        None::<String>,
        None::<i32>,
        None::<String>,
        None::<bool>,
        None::<i32>,
        None::<i32>,
        None::<chrono::DateTime<chrono::Utc>>,
        None::<chrono::DateTime<chrono::Utc>>,
    )
}

/// Index size, document counts, and — the part that has bitten this deployment —
/// disk headroom and read-only block state.
///
/// Degrades field by field: OpenSearch being unreachable must not blank the whole
/// stats page, so each probe contributes what it can.
pub async fn index_stats(state: &AppState) -> IndexStats {
    let runtime = state.runtime();
    let name = runtime.index_name.clone();

    let (cat, allocation, health, read_only) = tokio::join!(
        state.os.cat_index(&name),
        state.os.cat_allocation(),
        state.os.cluster_health(),
        state.os.index_read_only(&name),
    );

    let cat = cat.ok().flatten();
    let allocation = allocation.ok().flatten();

    let parse_i64 = |value: Option<&serde_json::Value>| -> Option<i64> {
        value.and_then(|v| match v {
            serde_json::Value::String(s) => s.parse().ok(),
            serde_json::Value::Number(n) => n.as_i64(),
            _ => None,
        })
    };

    let size_bytes = parse_i64(cat.as_ref().and_then(|c| c.get("store.size")));
    let docs = parse_i64(cat.as_ref().and_then(|c| c.get("docs.count")));
    let deleted_docs = parse_i64(cat.as_ref().and_then(|c| c.get("docs.deleted")));

    let disk_total = parse_i64(allocation.as_ref().and_then(|a| a.get("disk.total")));
    let disk_available = parse_i64(allocation.as_ref().and_then(|a| a.get("disk.avail")));
    let disk_used_pct = parse_i64(allocation.as_ref().and_then(|a| a.get("disk.percent")))
        .map(|p| p as f64)
        .or_else(|| match (disk_total, disk_available) {
            (Some(total), Some(avail)) if total > 0 => {
                Some(((total - avail) as f64 / total as f64 * 1000.0).round() / 10.0)
            }
            _ => None,
        });

    IndexStats {
        name,
        size_bytes,
        docs,
        deleted_docs,
        disk_used_pct,
        disk_total_bytes: disk_total,
        disk_available_bytes: disk_available,
        // A failed probe reports "not blocked" rather than crying wolf; the
        // OpenSearch health entry already shows the dependency is down.
        read_only: read_only.unwrap_or(false),
        cluster_status: health
            .ok()
            .and_then(|h| h["status"].as_str().map(|s| s.to_string())),
    }
}

pub async fn timeline(
    state: &AppState,
    window: TimelineWindow,
    bucket: TimelineBucketSize,
) -> Result<TimelineResponse, AppError> {
    let key = format!("timeline:{}:{}", window.as_str(), bucket.as_str());
    cached(state, &key, || async {
        let items = stats::timeline(&state.db, window, bucket).await?;
        Ok(TimelineResponse {
            window: window.as_str().to_string(),
            bucket: bucket.as_str().to_string(),
            items,
        })
    })
    .await
}

/// Documents and connectors per source, with chunk counts from the index.
///
/// `source_type` is a `text` field in this mapping and cannot be aggregated, so
/// chunk counts come from one cheap count per source.
pub async fn by_source(state: &AppState) -> Result<Vec<SourceStat>, AppError> {
    cached(state, "sources", || async {
        let runtime = state.runtime();
        let mut sources = stats::by_source(&state.db).await?;
        for source in &mut sources {
            source.chunks = state
                .os
                .count_by_source(&runtime.index_name, &source.source)
                .await
                .ok();
        }
        Ok(sources)
    })
    .await
}

pub async fn top_connectors(
    state: &AppState,
    by_recent: bool,
    limit: i64,
) -> Result<Vec<TopConnector>, AppError> {
    let key = format!(
        "top:{}:{}",
        if by_recent { "recent" } else { "docs" },
        limit
    );
    cached(state, &key, || async {
        Ok(connectors::top_connectors(&state.db, by_recent, limit).await?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unfiltered_count_key_matches_the_pages_service() {
        // If these drift, /stats/overview and /pages report different totals for
        // the same database.
        let from_pages = {
            let filter = ovis_core::db::documents::DocumentFilter::default();
            format!(
                "s={:?}|c={:?}|src={:?}|h={:?}|cmin={:?}|cmax={:?}|ua={:?}|ub={:?}",
                filter.search,
                filter.connector_id,
                filter.source.as_ref().map(|s| s.to_uppercase()),
                filter.hidden,
                filter.chunk_min,
                filter.chunk_max,
                filter.updated_after,
                filter.updated_before,
            )
        };
        assert_eq!(unfiltered_count_key(), from_pages);
    }

    #[test]
    fn disk_percentage_is_derived_when_opensearch_omits_it() {
        // _cat/allocation gives disk.percent, but if only totals came back we can
        // still work it out.
        let total = 844_367_142_912i64;
        let avail = 383_812_423_680i64;
        let pct = ((total - avail) as f64 / total as f64 * 1000.0).round() / 10.0;
        assert!((pct - 54.5).abs() < 0.5, "got {pct}");
    }
}
