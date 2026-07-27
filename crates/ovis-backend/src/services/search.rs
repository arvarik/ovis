//! Content search.
//!
//! Distinct from list filtering: `/pages?search=` matches titles and URLs in
//! Postgres, this searches the *text of the chunks* in OpenSearch and hydrates
//! the winners from Postgres. Two round-trips total, however many hits come back.

use std::collections::HashMap;
use std::time::Instant;

use ovis_core::api_types::{SearchHit, SearchMode, SearchResponse};
use ovis_core::db::documents;
use ovis_core::search::{SearchFilters, SearchRequest as OsSearchRequest};

use crate::error::AppError;
use crate::state::AppState;

/// The chunk index carries no connector field, so a connector filter can only be
/// applied after hydration. Over-fetch so the page can still be filled.
///
/// The floor matters more than the multiplier: at `limit=3` a bare 5× over-fetch
/// asks for 15 hits, and a connector holding even 6% of the corpus will often own
/// none of the global top 15 — which looks like "this connector has no matches"
/// rather than "we did not look far enough".
const CONNECTOR_OVERFETCH: i64 = 5;
const CONNECTOR_OVERFETCH_FLOOR: i64 = 200;
const CONNECTOR_OVERFETCH_CAP: i64 = 500;

pub struct SearchQuery {
    pub q: String,
    pub mode: SearchMode,
    pub connector_id: Option<i32>,
    pub source: Option<String>,
    pub include_hidden: bool,
    pub limit: i64,
    pub offset: i64,
}

pub async fn search(state: &AppState, query: SearchQuery) -> Result<SearchResponse, AppError> {
    let started = Instant::now();
    let runtime = state.runtime();

    if query.q.trim().is_empty() {
        return Err(AppError::BadRequest("q must not be empty".into()));
    }

    // Work out what we can actually serve, and record why if it is less than
    // what was asked for.
    let mut degraded: Option<String> = None;
    let mut vector = None;

    if query.mode.needs_embedding() {
        if !runtime.capabilities.knn_ready() {
            // The index declares a knn_vector field but no document populates
            // it (see IndexCapabilities). A kNN query would return zero hits in
            // 1 ms, which reads as "nothing matched" rather than "not supported".
            degraded = Some("no_knn_field".into());
        } else if let Some(embedder) = state.embed.as_ref() {
            match embedder
                .embed_query(&runtime.query_prefix, &query.q)
                .await
            {
                Ok(v) if v.len() as u32 == runtime.embedding_dim => vector = Some(v),
                Ok(v) => {
                    tracing::warn!(
                        got = v.len(),
                        want = runtime.embedding_dim,
                        "embedder returned the wrong dimension; falling back to keyword search"
                    );
                    degraded = Some("embedding_dim_mismatch".into());
                }
                Err(err) => {
                    tracing::warn!(error = %err, "embedding failed; falling back to keyword search");
                    degraded = Some("no_embedder".into());
                }
            }
        } else {
            degraded = Some("no_embedder".into());
        }
    }

    // A connector filter cannot be pushed into the index: chunks carry no
    // connector field. Narrow during hydration instead, over-fetching so the
    // page can still be filled.
    let connector_ids: Option<Vec<i32>> = query.connector_id.map(|id| vec![id]);
    let post_filtering = connector_ids.is_some();
    let os_limit = if post_filtering {
        (query.limit * CONNECTOR_OVERFETCH)
            .clamp(CONNECTOR_OVERFETCH_FLOOR, CONNECTOR_OVERFETCH_CAP)
    } else {
        query.limit
    };

    let os_request = OsSearchRequest {
        query: query.q.clone(),
        mode: query.mode,
        filters: SearchFilters {
            source: query.source.clone(),
            include_hidden: query.include_hidden,
        },
        limit: os_limit,
        offset: query.offset,
        vector,
        knn_field: runtime.capabilities.knn_field.clone(),
    };

    let raw = state.os.search(&runtime.index_name, &os_request).await?;

    let document_ids: Vec<String> = raw.hits.iter().map(|h| h.document_id.clone()).collect();
    let hydrated =
        documents::documents_by_ids(&state.db, &document_ids, connector_ids.as_deref()).await?;
    let by_id: HashMap<&str, &ovis_core::api_types::PageListItem> =
        hydrated.iter().map(|d| (d.id.as_str(), d)).collect();

    let mut items: Vec<SearchHit> = Vec::with_capacity(raw.hits.len());
    for hit in &raw.hits {
        // A hit with no Postgres row is an orphaned chunk. Skip it when a
        // connector filter is active (it cannot satisfy the filter); otherwise
        // keep it, flagged only by its absent metadata.
        let doc = by_id.get(hit.document_id.as_str());
        if post_filtering && doc.is_none() {
            continue;
        }
        items.push(SearchHit {
            document_id: hit.document_id.clone(),
            semantic_id: doc
                .map(|d| d.semantic_id.clone())
                .or_else(|| hit.semantic_identifier.clone()),
            link: doc.and_then(|d| d.link.clone()),
            score: hit.score,
            snippet: hit.snippet.clone(),
            chunk_index: hit.chunk_index,
            connector_id: doc.and_then(|d| d.connector_id),
            connector_name: doc.and_then(|d| d.connector_name.clone()),
            connector_source: doc
                .and_then(|d| d.connector_source.clone())
                .or_else(|| hit.source_type.clone()),
            chunk_count: doc.and_then(|d| d.chunk_count),
            updated_at: doc.map(|d| d.updated_at).or(hit.last_updated),
        });
        if items.len() as i64 >= query.limit {
            break;
        }
    }

    // With post-filtering the index's total counts documents we dropped, so it
    // is not the total for this result set. Say so rather than reporting a
    // number that does not match the list.
    let (total_hits, total_hits_exact) = if post_filtering {
        (items.len() as i64, false)
    } else {
        (raw.total, raw.total_exact)
    };
    if post_filtering && degraded.is_none() {
        degraded = Some("connector_filter_post_applied".into());
    }

    Ok(SearchResponse {
        items,
        mode: query.mode.as_str().to_string(),
        degraded,
        total_hits,
        total_hits_exact,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overfetch(limit: i64) -> i64 {
        (limit * CONNECTOR_OVERFETCH).clamp(CONNECTOR_OVERFETCH_FLOOR, CONNECTOR_OVERFETCH_CAP)
    }

    #[test]
    fn connector_overfetch_has_a_useful_floor_and_a_hard_cap() {
        // A small page must still look deep enough to find the connector's hits.
        assert_eq!(overfetch(3), CONNECTOR_OVERFETCH_FLOOR);
        assert_eq!(overfetch(20), CONNECTOR_OVERFETCH_FLOOR);
        assert_eq!(overfetch(50), 250);
        // Never unbounded, however large the requested page.
        assert_eq!(overfetch(100), CONNECTOR_OVERFETCH_CAP);
        assert_eq!(overfetch(10_000), CONNECTOR_OVERFETCH_CAP);
    }

    #[test]
    fn degradation_reasons_are_distinct_so_a_client_can_tell_them_apart() {
        let reasons = [
            "no_knn_field",
            "no_embedder",
            "embedding_dim_mismatch",
            "connector_filter_post_applied",
        ];
        let unique: std::collections::HashSet<&&str> = reasons.iter().collect();
        assert_eq!(unique.len(), reasons.len());
    }
}
