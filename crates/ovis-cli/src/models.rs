//! Display models for the current CLI and TUI.
//!
//! These used to live in `ovis-core`, where they were the *only* document shape
//! the whole workspace had. The backend redesign replaced them with the wire
//! types in `ovis_core::api_types`, which is what the API actually serialises.
//!
//! They live here now, scoped to the CLI, so `ovis-core` carries exactly one
//! document shape. The CLI's own redesign (see `redesign/cli/`) replaces these
//! with `ovis_core::api_types` consumed over HTTP, at which point this module
//! goes away — the CLI will not talk to Postgres at all.

use serde::{Deserialize, Serialize};

/// A document row, as the table and TUI renderers want it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentRecord {
    pub id: String,
    pub from_beginning: Option<bool>,
    pub semantic_id: String,
    pub link: Option<String>,
    pub doc_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub primary_owners: Option<Vec<String>>,
    pub secondary_owners: Option<Vec<String>>,
    pub metadata: serde_json::Value,
}

impl From<ovis_core::api_types::PageListItem> for DocumentRecord {
    fn from(item: ovis_core::api_types::PageListItem) -> Self {
        Self {
            id: item.id,
            from_beginning: None,
            semantic_id: item.semantic_id,
            link: item.link,
            // The effective recency timestamp, not the raw crawl one: on this
            // deployment `doc_updated_at` is null for all but ~1,500 of 1.65M
            // rows, so showing it raw would render an empty column.
            doc_updated_at: Some(item.updated_at),
            primary_owners: None,
            secondary_owners: None,
            metadata: item.metadata.unwrap_or(serde_json::Value::Null),
        }
    }
}

/// A chunk, as the inspector renders it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkRecord {
    pub chunk_id: usize,
    pub document_id: String,
    pub content: String,
    pub title: Option<String>,
    pub source_type: String,
    pub metadata: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<Vec<f32>>,
}

/// Connector summary, as the table renders it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorSummary {
    pub connector_id: i32,
    pub connector_name: String,
    pub connector_source: String,
    /// Now derived from the real cc-pair status rather than the hardcoded `false`
    /// the old query produced — 278 of 332 connectors here are PAUSED.
    pub disabled: bool,
    pub total_pages: i64,
    pub last_indexed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<ovis_core::api_types::ConnectorSummary> for ConnectorSummary {
    fn from(summary: ovis_core::api_types::ConnectorSummary) -> Self {
        Self {
            connector_id: summary.connector_id,
            connector_name: summary.name,
            connector_source: summary.source,
            disabled: !summary.status.eq_ignore_ascii_case("ACTIVE"),
            total_pages: summary.doc_count,
            last_indexed_at: summary.last_successful_index_time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ovis_core::api_types::{ConnectorSummary as WireSummary, PageListItem};

    #[test]
    fn a_paused_connector_is_shown_as_disabled() {
        let base = WireSummary {
            connector_id: 4,
            cc_pair_id: 4,
            name: "tildes".into(),
            source: "WEB".into(),
            status: "PAUSED".into(),
            parked: false,
            in_repeated_error_state: false,
            doc_count: 105_666,
            last_successful_index_time: None,
            refresh_freq_secs: Some(2_592_000),
            indexing_trigger: None,
            last_attempt: None,
        };
        let paused: ConnectorSummary = base.clone().into();
        assert!(paused.disabled, "the old code always reported false here");
        assert_eq!(paused.total_pages, 105_666);

        let active: ConnectorSummary = WireSummary {
            status: "ACTIVE".into(),
            ..base.clone()
        }
        .into();
        assert!(!active.disabled);

        // INITIAL_INDEXING is not ACTIVE, and should not read as fully healthy.
        let initial: ConnectorSummary = WireSummary {
            status: "INITIAL_INDEXING".into(),
            ..base
        }
        .into();
        assert!(initial.disabled);
    }

    #[test]
    fn document_conversion_uses_the_effective_timestamp() {
        let item = PageListItem {
            id: "https://example.com/a".into(),
            semantic_id: "A".into(),
            link: Some("https://example.com/a".into()),
            updated_at: "2026-07-20T00:00:00Z".parse().unwrap(),
            // Null, as it is for nearly every row on this deployment.
            doc_updated_at: None,
            last_modified: "2026-07-20T00:00:00Z".parse().unwrap(),
            chunk_count: Some(14),
            boost: 0,
            hidden: false,
            connector_id: Some(4),
            connector_name: Some("tildes".into()),
            connector_source: Some("WEB".into()),
            metadata: Some(serde_json::json!({ "k": "v" })),
        };
        let record: DocumentRecord = item.into();
        assert!(
            record.doc_updated_at.is_some(),
            "the date column must not be blank just because doc_updated_at is null"
        );
        assert_eq!(record.metadata["k"], "v");
    }

    #[test]
    fn absent_metadata_becomes_null_not_a_panic() {
        let item = PageListItem {
            id: "x".into(),
            semantic_id: "x".into(),
            link: None,
            updated_at: "2026-07-20T00:00:00Z".parse().unwrap(),
            doc_updated_at: None,
            last_modified: "2026-07-20T00:00:00Z".parse().unwrap(),
            chunk_count: None,
            boost: 0,
            hidden: false,
            connector_id: None,
            connector_name: None,
            connector_source: None,
            metadata: None,
        };
        let record: DocumentRecord = item.into();
        assert_eq!(record.metadata, serde_json::Value::Null);
    }
}
