//! Page list, detail, chunks, edit and delete.

use std::sync::Arc;

use ovis_core::api_types::{
    BatchDeleteFailure, BatchDeleteResponse, ChunkItem, ChunksResponse, DeleteOutcome,
    ListResponse, PageDetail, PageListItem, PagePatch, PatchResponse,
};
use ovis_core::cursor::{Cursor, SortOrder};
use ovis_core::db::documents::{
    self, ConnectorPlan, DocumentFilter, DocumentUpdate, Position, MAX_OFFSET_DEPTH,
};
use ovis_core::CoreError;

use crate::error::AppError;
use crate::state::AppState;

/// Documents per Postgres round-trip while streaming. Small enough that the
/// first SSE event goes out immediately, large enough that a 10,000-row stream
/// is 50 queries rather than 10,000.
pub const STREAM_BATCH: i64 = 200;

/// Tags returned with a document detail. A pathological document must not be
/// able to turn one response into a megabyte of tag JSON.
const DETAIL_TAG_LIMIT: i64 = 200;

/// Where a list request starts, before the cursor has been validated.
#[derive(Debug, Clone)]
pub enum RequestedPosition {
    Page(i64),
    Cursor(String),
}

pub struct ListRequest {
    pub filter: DocumentFilter,
    pub sort: SortOrder,
    pub position: RequestedPosition,
    pub limit: i64,
}

/// One page of documents, with its total.
///
/// The row fetch and the count run **concurrently**: they are independent
/// queries, and serialising them would make every list response as slow as the
/// sum rather than the slower of the two.
pub async fn list(
    state: &AppState,
    request: ListRequest,
) -> Result<ListResponse<PageListItem>, AppError> {
    let runtime = state.runtime();
    runtime.requires_column("document", "chunk_count")?;

    let cursor = match &request.position {
        RequestedPosition::Cursor(token) => Some(Cursor::decode(token, request.sort)?),
        RequestedPosition::Page(_) => None,
    };
    let page = match &request.position {
        RequestedPosition::Page(page) => Some((*page).max(1)),
        RequestedPosition::Cursor(_) => None,
    };
    let position = match &cursor {
        Some(cursor) => Position::After(cursor),
        // The offset comes from the *requested* limit, not the `limit + 1` used
        // below to detect a next page.
        None => Position::Offset(documents::offset_for_page(page.unwrap_or(1), request.limit)),
    };

    // One extra row tells us whether another page exists without a second query.
    let fetch_limit = request.limit + 1;
    let plan = documents::plan_connector_filter(&state.db, &request.filter).await?;

    let rows_fut = documents::list_documents(
        &state.db,
        &request.filter,
        &plan,
        request.sort,
        position,
        fetch_limit,
    );
    let total_fut = resolve_total(state, &request.filter, &plan);

    let (rows, (total, total_exact)) = tokio::try_join!(rows_fut, total_fut)?;

    let mut items = rows;
    let has_more = items.len() > request.limit as usize;
    if has_more {
        items.truncate(request.limit as usize);
    }

    let next_cursor = has_more
        .then(|| {
            items
                .last()
                .map(|item| Cursor::after(request.sort, item).encode())
        })
        .flatten();

    Ok(ListResponse {
        items,
        total,
        total_exact,
        page,
        limit: request.limit,
        next_cursor,
        has_more,
    })
}

/// Total matching rows, and whether the number is exact.
///
/// An exact `count(*)` over the unfiltered 1.65M-row table takes ~130 ms, which
/// would dominate an otherwise sub-millisecond list response. So the unfiltered
/// grand total is answered from the planner's estimate while an exact count runs
/// in the background and takes over once it lands. Filtered counts are exact —
/// they are the ones a user is likely to read closely.
async fn resolve_total(
    state: &AppState,
    filter: &DocumentFilter,
    plan: &ConnectorPlan,
) -> Result<(i64, bool), CoreError> {
    let key = cache_key(filter);

    if let Some(cached) = state.caches.counts.get(&key).await {
        return Ok((cached, true));
    }

    if !filter.is_unfiltered() {
        let count = documents::count_documents(&state.db, filter, plan).await?;
        state.caches.counts.insert(key, count).await;
        return Ok((count, true));
    }

    let estimate = documents::estimate_total_documents(&state.db).await?;
    spawn_exact_count(state.clone(), filter.clone(), plan.clone(), key).await;
    Ok((estimate, false))
}

/// Kick off one background exact count per filter key.
async fn spawn_exact_count(
    state: AppState,
    filter: DocumentFilter,
    plan: ConnectorPlan,
    key: String,
) {
    // The in-flight marker collapses a burst of requests into a single count.
    if state.caches.count_inflight.get(&key).await.is_some() {
        return;
    }
    state.caches.count_inflight.insert(key.clone(), ()).await;

    tokio::spawn(async move {
        match documents::count_documents(&state.db, &filter, &plan).await {
            Ok(count) => {
                tracing::debug!(count, "refreshed the exact document total");
                state.caches.counts.insert(key.clone(), count).await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "background exact count failed");
            }
        }
        state.caches.count_inflight.invalidate(&key).await;
    });
}

fn cache_key(filter: &DocumentFilter) -> String {
    // Cheap, stable, and collision-free for the filter shape.
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
}

/// Reject an offset page beyond the bound before touching the database.
pub fn validate_page_depth(page: i64, limit: i64) -> Result<(), AppError> {
    let offset = documents::offset_for_page(page, limit);
    if offset.saturating_add(limit) > MAX_OFFSET_DEPTH {
        return Err(AppError::BadRequest(format!(
            "page {page} at limit {limit} exceeds the {MAX_OFFSET_DEPTH}-row offset bound; \
             follow `next_cursor` instead, or narrow the filter"
        )));
    }
    Ok(())
}

/// Stream documents in keyset batches.
///
/// Batching by cursor rather than holding one long-lived `fetch` stream matters
/// for pool health: a slow client would otherwise pin a Postgres connection for
/// the entire life of the stream. Between batches the connection goes back.
pub async fn stream_batch(
    state: &AppState,
    filter: &DocumentFilter,
    plan: &ConnectorPlan,
    sort: SortOrder,
    after: Option<&Cursor>,
    limit: i64,
) -> Result<Vec<PageListItem>, AppError> {
    let position = match after {
        Some(cursor) => Position::After(cursor),
        None => Position::Offset(0),
    };
    Ok(documents::list_documents(&state.db, filter, plan, sort, position, limit).await?)
}

/// Metadata detail for one document.
///
/// A missing Postgres row is not automatically a 404: the index may still hold
/// chunks, which means the document was deleted underneath Onyx or an index
/// delete outlived its row. That case is reported with `pg_row: false` so a
/// client can badge it, rather than being synthesised into something that looks
/// like a healthy document.
pub async fn detail(state: &AppState, id: &str) -> Result<PageDetail, AppError> {
    let runtime = state.runtime();
    let index = runtime.index_name.clone();

    let row_fut = documents::get_document(&state.db, id);
    let tags_fut = documents::get_document_tags(&state.db, id, DETAIL_TAG_LIMIT);
    let (row, tags) = tokio::try_join!(row_fut, tags_fut)?;

    match row {
        Some(mut detail) => {
            detail.tags = tags;
            Ok(detail)
        }
        None => {
            // No row: ask the index whether chunks are orphaned here.
            let (_, total_chunks, _) = state
                .os
                .document_chunks(&index, id, None, 1, false)
                .await
                .unwrap_or((Vec::new(), 0, None));

            if total_chunks == 0 {
                return Err(AppError::NotFound {
                    what: "document",
                    id: id.to_string(),
                });
            }

            tracing::warn!(
                document_id = %id,
                total_chunks,
                "index holds chunks for a document with no Postgres row"
            );
            Ok(orphaned_detail(id, total_chunks, tags))
        }
    }
}

fn orphaned_detail(
    id: &str,
    total_chunks: i64,
    tags: Vec<ovis_core::api_types::TagKv>,
) -> PageDetail {
    let now = chrono::Utc::now();
    PageDetail {
        item: PageListItem {
            id: id.to_string(),
            semantic_id: id.to_string(),
            link: None,
            updated_at: now,
            doc_updated_at: None,
            last_modified: now,
            chunk_count: Some(total_chunks.min(i32::MAX as i64) as i32),
            boost: 0,
            hidden: false,
            connector_id: None,
            connector_name: None,
            connector_source: None,
            metadata: None,
        },
        primary_owners: None,
        secondary_owners: None,
        content_hash: None,
        from_ingestion_api: None,
        last_synced: None,
        cc_pair_id: None,
        cc_pair_status: None,
        tags,
        pg_row: false,
        recrawl_risk: false,
    }
}

/// One page of a document's chunks.
pub async fn chunks(
    state: &AppState,
    id: &str,
    after: Option<i64>,
    limit: i64,
    include_content: bool,
) -> Result<ChunksResponse, AppError> {
    let runtime = state.runtime();
    let (items, total_chunks, next_after) = state
        .os
        .document_chunks(&runtime.index_name, id, after, limit, include_content)
        .await?;

    if items.is_empty() && total_chunks == 0 && after.is_none() {
        // Distinguish "no chunks" from "no such document" using Postgres.
        if documents::get_document(&state.db, id).await?.is_none() {
            return Err(AppError::NotFound {
                what: "document",
                id: id.to_string(),
            });
        }
    }

    Ok(ChunksResponse {
        items,
        total_chunks,
        next_after,
        embedding_model: runtime.embedding_model.clone(),
        embedding_dim: runtime.embedding_dim,
    })
}

/// One chunk's real stored vector.
///
/// Returns `501 NOT_AVAILABLE` when this index stores no readable per-chunk
/// vector, rather than inventing one — the previous UI displayed fabricated
/// vectors, which is worse than displaying none.
pub async fn chunk_vector(
    state: &AppState,
    id: &str,
    chunk_index: i64,
) -> Result<ovis_core::api_types::ChunkVector, AppError> {
    let runtime = state.runtime();
    let field = runtime
        .capabilities
        .source_vector_field
        .as_deref()
        .ok_or_else(|| {
            AppError::NotAvailable(format!(
                "index '{}' exposes no readable per-chunk vector field",
                runtime.index_name
            ))
        })?;

    let vector = state
        .os
        .chunk_vector(&runtime.index_name, id, chunk_index, field)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "chunk vector",
            id: format!("{id}__{chunk_index}"),
        })?;

    Ok(ovis_core::api_types::ChunkVector {
        dim: vector.len(),
        model: runtime.embedding_model.clone(),
        vector,
    })
}

/// Full reconstructed text, chunks joined in order.
pub async fn text(state: &AppState, id: &str) -> Result<String, AppError> {
    let runtime = state.runtime();
    let text = state.os.document_text(&runtime.index_name, id).await?;
    if text.is_empty() {
        // Either the document does not exist or it genuinely has no content;
        // Postgres decides which.
        if documents::get_document(&state.db, id).await?.is_none() {
            return Err(AppError::NotFound {
                what: "document",
                id: id.to_string(),
            });
        }
    }
    Ok(text)
}

/// Apply a patch.
///
/// `boost` and `hidden` go through the Onyx API when a token is configured, so
/// Onyx performs its own index bookkeeping. Without one, they are written
/// directly and OVIS syncs the corresponding index fields itself — reported in
/// `boost_hidden_via` so the caller knows which path ran.
pub async fn patch(
    state: &AppState,
    id: &str,
    patch: PagePatch,
) -> Result<PatchResponse, AppError> {
    if patch.is_empty() {
        return Err(AppError::BadRequest(
            "nothing to change: supply at least one of semantic_id, boost, hidden, \
             metadata_merge"
                .into(),
        ));
    }
    if let Some(merge) = &patch.metadata_merge {
        if !merge.is_object() {
            return Err(AppError::BadRequest(
                "metadata_merge must be a JSON object; it is shallow-merged into doc_metadata"
                    .into(),
            ));
        }
    }
    if let Some(boost) = patch.boost {
        // Onyx's boost is a small relevance nudge, not a score.
        if !(-100..=100).contains(&boost) {
            return Err(AppError::BadRequest(
                "boost must be between -100 and 100".into(),
            ));
        }
    }

    let runtime = state.runtime();
    // Confirm the document exists before mutating anything.
    if documents::get_document(&state.db, id).await?.is_none() {
        return Err(AppError::NotFound {
            what: "document",
            id: id.to_string(),
        });
    }

    let mut boost_hidden_via: Option<String> = None;
    let touches_flags = patch.boost.is_some() || patch.hidden.is_some();

    // Prefer Onyx for boost/hidden so its own index sync runs.
    let mut sql_update = DocumentUpdate {
        semantic_id: patch.semantic_id.clone(),
        boost: patch.boost,
        hidden: patch.hidden,
        metadata_merge: patch.metadata_merge.clone(),
    };

    if touches_flags {
        if let Some(onyx) = state.onyx.as_ref() {
            if let Some(boost) = patch.boost {
                onyx.set_doc_boost(id, boost).await?;
            }
            if let Some(hidden) = patch.hidden {
                onyx.set_doc_hidden(id, hidden).await?;
            }
            // Onyx owns these now; do not also write them.
            sql_update.boost = None;
            sql_update.hidden = None;
            boost_hidden_via = Some("onyx_api".into());
            tracing::info!(
                document_id = %id,
                boost = ?patch.boost,
                hidden = ?patch.hidden,
                "applied boost/hidden via the Onyx API"
            );
        } else {
            boost_hidden_via = Some("direct_sql".into());
        }
    }

    if !sql_update.is_empty() {
        let affected = documents::update_document(&state.db, id, &sql_update).await?;
        if affected == 0 {
            return Err(AppError::NotFound {
                what: "document",
                id: id.to_string(),
            });
        }
    }

    // Propagate to the index: the title always, flags only when Onyx did not.
    let mut index_synced = true;
    if let Some(title) = patch.semantic_id.as_deref() {
        match state
            .os
            .update_document_title(&runtime.index_name, id, title)
            .await
        {
            Ok(updated) => {
                tracing::info!(document_id = %id, chunks_updated = updated, "synced title to the index")
            }
            Err(err) => {
                index_synced = false;
                tracing::warn!(document_id = %id, error = %err, "title index sync failed");
            }
        }
    }
    if boost_hidden_via.as_deref() == Some("direct_sql") {
        if let Err(err) = state
            .os
            .update_document_flags(&runtime.index_name, id, patch.hidden, patch.boost)
            .await
        {
            index_synced = false;
            tracing::warn!(document_id = %id, error = %err, "flag index sync failed");
        }
    }

    state.caches.invalidate_document_scoped().await;

    let mut detail = detail(state, id).await?;
    detail.tags = documents::get_document_tags(&state.db, id, DETAIL_TAG_LIMIT).await?;

    Ok(PatchResponse {
        detail,
        index_synced,
        boost_hidden_via,
    })
}

/// Delete one document from Postgres and the index.
pub async fn delete(state: &AppState, id: &str) -> Result<DeleteOutcome, AppError> {
    let runtime = state.runtime();
    runtime.delete_is_safe()?;

    let outcome =
        documents::delete_document_cascading(&state.db, &state.os, &runtime.index_name, id).await?;

    tracing::info!(
        document_id = %id,
        chunks_deleted = outcome.chunks_deleted,
        index_cleanup_pending = outcome.index_cleanup_pending,
        recrawl_risk = outcome.recrawl_risk,
        "deleted a document"
    );
    state.caches.invalidate_document_scoped().await;
    Ok(outcome)
}

/// Delete a batch, chunked so each group shares one index call.
///
/// Reports per-item outcomes: `success` is true only when nothing failed, and a
/// failed id appears in `failed` with its code. The old implementation always
/// reported `success: true` and dropped failures on the floor.
pub async fn batch_delete(
    state: &AppState,
    ids: Vec<String>,
) -> Result<BatchDeleteResponse, AppError> {
    let runtime = state.runtime();
    runtime.delete_is_safe()?;

    if ids.is_empty() {
        return Err(AppError::BadRequest(
            "document_ids must contain at least one id".into(),
        ));
    }
    if ids.len() > state.cfg.batch_delete_max {
        return Err(AppError::BadRequest(format!(
            "{} ids exceeds the batch limit of {}; split the request",
            ids.len(),
            state.cfg.batch_delete_max
        )));
    }

    let mut deleted = 0usize;
    let mut chunks_deleted = 0u64;
    let mut failed: Vec<BatchDeleteFailure> = Vec::new();
    let mut index_cleanup_pending = 0usize;

    // Postgres per document (each needs its own FK sweep transaction), then one
    // index delete per group of 100.
    for group in ids.chunks(100) {
        let mut committed: Vec<String> = Vec::with_capacity(group.len());
        for id in group {
            match documents::delete_document_pg_only(&state.db, id).await {
                Ok(()) => committed.push(id.clone()),
                Err(err) => {
                    let app: AppError = err.into();
                    failed.push(BatchDeleteFailure {
                        id: id.clone(),
                        code: app.code().to_string(),
                    });
                    if app.is_server_side() {
                        tracing::error!(document_id = %id, detail = %app.log_detail(), "batch delete failed");
                    }
                }
            }
        }

        if committed.is_empty() {
            continue;
        }
        match state
            .os
            .delete_chunks_for(&runtime.index_name, &committed)
            .await
        {
            Ok(n) => {
                chunks_deleted += n;
                deleted += committed.len();
            }
            Err(err) => {
                // Postgres is already committed; queue every id in the group.
                tracing::warn!(
                    count = committed.len(),
                    error = %err,
                    "batch index cleanup failed; queueing for retry"
                );
                for id in &committed {
                    let _ =
                        ovis_core::db::pending_deletes::enqueue(&state.db, id, &err.to_string())
                            .await;
                }
                index_cleanup_pending += committed.len();
                deleted += committed.len();
            }
        }
    }

    state.caches.invalidate_document_scoped().await;

    tracing::info!(
        deleted,
        failed = failed.len(),
        chunks_deleted,
        index_cleanup_pending,
        "batch delete completed"
    );

    Ok(BatchDeleteResponse {
        success: failed.is_empty(),
        deleted,
        chunks_deleted,
        failed,
        index_cleanup_pending,
    })
}

/// Chunk items joined into one text blob, for the `text/plain` endpoint.
pub fn join_chunk_text(items: &[ChunkItem]) -> String {
    items
        .iter()
        .filter_map(|c| c.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Shared clamp for list limits.
pub fn clamp_limit(requested: Option<i64>, default: i64, max: i64) -> i64 {
    requested.unwrap_or(default).clamp(1, max.max(1))
}

/// Cached connector summaries, used by both `/connectors` and the stats routes.
pub async fn cached_connectors(
    state: &AppState,
) -> Result<Arc<Vec<ovis_core::api_types::ConnectorSummary>>, AppError> {
    if let Some(cached) = state.caches.connectors.get(&()).await {
        return Ok(cached);
    }
    let summaries = Arc::new(ovis_core::db::connectors::list_summaries(&state.db).await?);
    state.caches.connectors.insert((), summaries.clone()).await;
    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_separate_distinct_filters_and_unify_source_casing() {
        let a = DocumentFilter {
            search: Some("tax".into()),
            ..Default::default()
        };
        let b = DocumentFilter {
            search: Some("vat".into()),
            ..Default::default()
        };
        assert_ne!(cache_key(&a), cache_key(&b));

        let upper = DocumentFilter {
            source: Some("WEB".into()),
            ..Default::default()
        };
        let lower = DocumentFilter {
            source: Some("web".into()),
            ..Default::default()
        };
        assert_eq!(
            cache_key(&upper),
            cache_key(&lower),
            "source casing must not fragment the count cache"
        );

        assert_ne!(
            cache_key(&DocumentFilter::default()),
            cache_key(&DocumentFilter {
                hidden: Some(false),
                ..Default::default()
            }),
            "an unfiltered total and a hidden=false total are different numbers"
        );
    }

    #[test]
    fn chunk_bound_filters_do_not_collide_across_positions() {
        let min_only = DocumentFilter {
            chunk_min: Some(11),
            ..Default::default()
        };
        let max_only = DocumentFilter {
            chunk_max: Some(11),
            ..Default::default()
        };
        assert_ne!(cache_key(&min_only), cache_key(&max_only));
    }

    #[test]
    fn page_depth_bound_is_enforced_before_any_query() {
        assert!(validate_page_depth(1, 50).is_ok());
        assert!(validate_page_depth(1000, 50).is_ok());
        let err = validate_page_depth(100_000, 50).unwrap_err();
        assert_eq!(err.code(), "BAD_REQUEST");
        assert!(err.client_message().contains("next_cursor"));
    }

    #[test]
    fn page_depth_bound_accounts_for_the_limit() {
        // The bound is on rows skipped plus rows returned, so a big limit hits it
        // at a lower page number.
        assert!(validate_page_depth(101, 500).is_err());
        assert!(validate_page_depth(100, 500).is_ok());
    }

    #[test]
    fn limit_clamping_rejects_zero_and_negatives_and_caps_at_max() {
        assert_eq!(clamp_limit(None, 50, 500), 50);
        assert_eq!(clamp_limit(Some(0), 50, 500), 1);
        assert_eq!(clamp_limit(Some(-10), 50, 500), 1);
        assert_eq!(clamp_limit(Some(10_000), 50, 500), 500);
        assert_eq!(clamp_limit(Some(200), 50, 500), 200);
    }

    #[test]
    fn joining_chunk_text_skips_chunks_fetched_without_content() {
        let items = vec![
            ChunkItem {
                chunk_index: 0,
                content: Some("first".into()),
                blurb: None,
                title: None,
                semantic_identifier: None,
                source_type: None,
                token_estimate: Some(1),
                source_links: None,
                last_updated: None,
                hidden: None,
                metadata_list: None,
            },
            ChunkItem {
                chunk_index: 1,
                content: None,
                blurb: None,
                title: None,
                semantic_identifier: None,
                source_type: None,
                token_estimate: None,
                source_links: None,
                last_updated: None,
                hidden: None,
                metadata_list: None,
            },
            ChunkItem {
                chunk_index: 2,
                content: Some("third".into()),
                blurb: None,
                title: None,
                semantic_identifier: None,
                source_type: None,
                token_estimate: Some(1),
                source_links: None,
                last_updated: None,
                hidden: None,
                metadata_list: None,
            },
        ];
        assert_eq!(join_chunk_text(&items), "first\n\nthird");
    }

    #[test]
    fn an_orphan_detail_is_flagged_rather_than_dressed_up() {
        let detail = orphaned_detail("https://example.com/a", 14, Vec::new());
        assert!(!detail.pg_row, "clients must be able to badge this");
        assert_eq!(detail.item.chunk_count, Some(14));
        assert!(!detail.recrawl_risk);
        assert_eq!(detail.item.id, "https://example.com/a");
        assert!(detail.item.metadata.is_none(), "no invented metadata");
    }

    #[test]
    fn stream_batch_size_keeps_the_first_event_prompt() {
        // 10,000 rows at this batch size is 50 queries; the first event goes out
        // after the first one.
        assert_eq!(STREAM_BATCH, 200);
        assert_eq!(10_000 / STREAM_BATCH, 50);
    }
}
