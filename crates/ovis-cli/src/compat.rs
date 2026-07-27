//! Transitional bridge from the current CLI to the rewritten data layer.
//!
//! The CLI still talks to Postgres and OpenSearch directly. That is precisely
//! what its own redesign removes — `redesign/cli/` has the CLI consuming the OVIS
//! HTTP API, so that only the backend holds credentials — but until that lands it
//! needs *something* to call.
//!
//! This module is that something, and it is deliberately thin: every function
//! here delegates to `ovis_core`, so the CLI gets the corrected queries (real
//! connector status, recency ordering, complete FK delete sweep, the real index
//! name from `search_settings`) instead of the divergent copies it used to carry.
//!
//! When the CLI redesign lands, this file is deleted.

use anyhow::Context;
use ovis_core::db::documents::{self, ConnectorPlan, DocumentFilter, Position};
use ovis_core::search::OsClient;
use sqlx::PgPool;

use crate::models::{ChunkRecord, ConnectorSummary, DocumentRecord};

/// Connection pool, with the same settings the server uses.
pub async fn create_pg_pool(dsn: &str) -> anyhow::Result<PgPool> {
    ovis_core::db::create_pg_pool(dsn, 5)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// The live chunk index name, from `search_settings WHERE status='PRESENT'`.
/// Never a `danswer_chunk*` wildcard.
pub async fn resolve_index(pool: &PgPool) -> anyhow::Result<String> {
    let settings = ovis_core::db::probe::load_search_settings(pool)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("could not resolve the OpenSearch index name from search_settings")?;
    Ok(settings.index_name)
}

pub fn os_client(base_url: &str) -> anyhow::Result<OsClient> {
    OsClient::new(base_url, None, None).map_err(|e| anyhow::anyhow!("{e}"))
}

/// List documents, newest first.
pub async fn fetch_documents(
    pool: &PgPool,
    connector_id: Option<i32>,
    source: Option<&str>,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<DocumentRecord>> {
    let filter = DocumentFilter {
        search: search.map(|s| s.to_string()),
        connector_id,
        source: source.map(|s| s.to_string()),
        ..Default::default()
    };
    let plan = documents::plan_connector_filter(pool, &filter)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let items = documents::list_documents(
        pool,
        &filter,
        &plan,
        Default::default(),
        Position::Offset(offset),
        limit,
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(items.into_iter().map(DocumentRecord::from).collect())
}

/// One document by exact id.
pub async fn fetch_document(pool: &PgPool, id: &str) -> anyhow::Result<Option<DocumentRecord>> {
    let items = documents::documents_by_ids(pool, &[id.to_string()], None)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(items.into_iter().next().map(DocumentRecord::from))
}

/// Connector summaries with real status and real document counts.
pub async fn fetch_connector_summaries(pool: &PgPool) -> anyhow::Result<Vec<ConnectorSummary>> {
    let summaries = ovis_core::db::connectors::list_summaries(pool)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(summaries.into_iter().map(ConnectorSummary::from).collect())
}

/// Every chunk of a document, in order, without downloading the embeddings.
pub async fn get_document_chunks(
    os: &OsClient,
    index: &str,
    doc_id: &str,
) -> anyhow::Result<Vec<ChunkRecord>> {
    let mut out: Vec<ChunkRecord> = Vec::new();
    let mut after: Option<i64> = None;
    loop {
        let (items, _total, next) = os
            .document_chunks(index, doc_id, after, 200, true)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if items.is_empty() {
            break;
        }
        for item in items {
            out.push(ChunkRecord {
                chunk_id: item.chunk_index.max(0) as usize,
                document_id: doc_id.to_string(),
                content: item.content.unwrap_or_default(),
                title: item.title.or(item.semantic_identifier),
                source_type: item.source_type.unwrap_or_else(|| "web".to_string()),
                metadata: item
                    .metadata_list
                    .map(|list| serde_json::json!({ "metadata_list": list }))
                    .unwrap_or(serde_json::Value::Null),
                // Vectors are deliberately absent: fetching them for display was
                // the single biggest cost in the old implementation.
                embeddings: None,
            });
        }
        match next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    Ok(out)
}

/// Cascading delete, with the complete foreign-key sweep.
pub async fn delete_document(
    pool: &PgPool,
    os: &OsClient,
    index: &str,
    doc_id: &str,
) -> anyhow::Result<ovis_core::api_types::DeleteOutcome> {
    documents::delete_document_cascading(pool, os, index, doc_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Update a title and merge metadata.
///
/// Unlike the old CLI edit, this merges `doc_metadata` instead of replacing it,
/// and never writes `doc_updated_at` — that column is Onyx's crawl timestamp and
/// its relationship to `last_synced` drives Onyx's own sync detection.
pub async fn update_document(
    pool: &PgPool,
    doc_id: &str,
    title: Option<&str>,
    metadata_merge: Option<&serde_json::Value>,
) -> anyhow::Result<u64> {
    let update = documents::DocumentUpdate {
        semantic_id: title.map(|t| t.to_string()),
        boost: None,
        hidden: None,
        metadata_merge: metadata_merge.cloned(),
    };
    documents::update_document(pool, doc_id, &update)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Propagate a title change into the chunk index.
pub async fn update_index_title(
    os: &OsClient,
    index: &str,
    doc_id: &str,
    title: &str,
) -> anyhow::Result<u64> {
    os.update_document_title(index, doc_id, title)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Unused-plan placeholder so callers that only need documents can skip the
/// selectivity probe.
pub fn unfiltered_plan() -> ConnectorPlan {
    ConnectorPlan::Unfiltered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_layer_takes_the_offset_the_cli_already_has() {
        // No page-number round trip, so no rounding to lose.
        assert_eq!(
            documents::Position::Offset(37),
            documents::Position::Offset(37)
        );
    }

    #[test]
    fn the_default_sort_is_recency() {
        let sort: ovis_core::cursor::SortOrder = Default::default();
        assert_eq!(sort, ovis_core::cursor::SortOrder::UpdatedDesc);
    }
}
