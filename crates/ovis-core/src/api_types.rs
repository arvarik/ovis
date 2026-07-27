//! Wire types for the OVIS HTTP API v1.
//!
//! These live in `ovis-core` rather than in the backend so the CLI can consume
//! the API with the *same* structs the server serialises — a shape change that
//! breaks a client is then a compile error, not a runtime surprise.
//!
//! Timestamps are RFC3339 UTC (chrono's serde impl). Optional fields are
//! serialised as `null` rather than omitted, so clients can rely on the key
//! being present.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Pagination envelope
// ---------------------------------------------------------------------------

/// Uniform list envelope used by every paginated endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListResponse<T> {
    pub items: Vec<T>,
    /// Total rows matching the filter. See `total_exact`.
    pub total: i64,
    /// `false` when `total` is a planner estimate (`pg_class.reltuples`) rather
    /// than a counted value. Only ever `false` for the unfiltered grand total,
    /// where an exact `count(*)` over 1.65M rows is too slow to do inline; an
    /// exact count is computed in the background and takes over once warm.
    pub total_exact: bool,
    /// 1-based page number for offset pagination; `null` when the request used a
    /// cursor.
    pub page: Option<i64>,
    pub limit: i64,
    /// Opaque keyset token for the next page. `null` when there is no next page.
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

/// One row of `GET /pages`. Sourced entirely from Postgres — the list path makes
/// zero OpenSearch calls (`chunk_count` comes from `document.chunk_count`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageListItem {
    /// The document id, which for web-crawled content *is* the URL.
    pub id: String,
    /// Title, as Onyx derived it.
    pub semantic_id: String,
    pub link: Option<String>,
    /// Effective recency: `COALESCE(doc_updated_at, last_modified)`. Never null,
    /// and it is exactly what `sort=updated_*` orders by.
    pub updated_at: DateTime<Utc>,
    /// Onyx's crawl-reported document timestamp. Null for the overwhelming
    /// majority of rows in this deployment (1,650,551 of 1,652,044 as measured),
    /// which is why `updated_at` exists.
    pub doc_updated_at: Option<DateTime<Utc>>,
    /// Onyx's row-touched timestamp. `NOT NULL` in the schema.
    pub last_modified: DateTime<Utc>,
    /// Number of chunks in the search index, per Postgres. `null` means Onyx has
    /// not recorded a count for this document yet — it is *not* the same as 0,
    /// and `chunk_min`/`chunk_max` filters exclude such rows.
    pub chunk_count: Option<i32>,
    pub boost: i32,
    pub hidden: bool,
    /// Attribution to the lowest-numbered connector that indexed this document.
    /// Deterministic; a document can belong to several connectors (3.2k do).
    pub connector_id: Option<i32>,
    pub connector_name: Option<String>,
    /// As stored by Onyx, i.e. upper-case (`WEB`, `GITHUB`, …). The `source`
    /// query parameter is matched case-insensitively.
    pub connector_source: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagKv {
    pub key: String,
    pub value: String,
}

/// `GET /pages/{id}` — a fast metadata-only view. No chunk content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageDetail {
    #[serde(flatten)]
    pub item: PageListItem,
    pub primary_owners: Option<Vec<String>>,
    pub secondary_owners: Option<Vec<String>>,
    pub content_hash: Option<String>,
    pub from_ingestion_api: Option<bool>,
    pub last_synced: Option<DateTime<Utc>>,
    pub cc_pair_id: Option<i32>,
    pub cc_pair_status: Option<String>,
    pub tags: Vec<TagKv>,
    /// `false` when there is no `document` row but the index still holds chunks —
    /// i.e. orphaned chunks. Clients badge this rather than pretending the
    /// document is fine.
    pub pg_row: bool,
    /// `true` when the owning cc-pair is ACTIVE or INITIAL_INDEXING, so a delete
    /// is liable to be undone by the next scheduled refresh.
    pub recrawl_risk: bool,
}

/// One chunk of `GET /pages/{id}/chunks`. Vectors are never included here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkItem {
    pub chunk_index: i64,
    pub content: Option<String>,
    pub blurb: Option<String>,
    pub title: Option<String>,
    pub semantic_identifier: Option<String>,
    pub source_type: Option<String>,
    /// Whitespace-delimited word count of `content`. Honestly named: this is a
    /// heuristic, not a tokeniser result.
    pub token_estimate: Option<i64>,
    pub source_links: Option<serde_json::Value>,
    pub last_updated: Option<DateTime<Utc>>,
    pub hidden: Option<bool>,
    pub metadata_list: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunksResponse {
    pub items: Vec<ChunkItem>,
    /// Total chunks the index holds for this document (`track_total_hits`), not
    /// the length of `items`.
    pub total_chunks: i64,
    /// `search_after` value for the next page, or `null` at the end.
    pub next_after: Option<i64>,
    pub embedding_model: String,
    pub embedding_dim: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkVector {
    pub dim: usize,
    pub model: String,
    pub vector: Vec<f32>,
}

/// Body of `PATCH /pages/{id}`. Every field is optional; absent means unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PagePatch {
    pub semantic_id: Option<String>,
    pub boost: Option<i32>,
    pub hidden: Option<bool>,
    /// Shallow-merged into `doc_metadata` (top-level keys replace). Never
    /// replaces the whole object — the old CLI edit stomped it.
    pub metadata_merge: Option<serde_json::Value>,
}

impl PagePatch {
    pub fn is_empty(&self) -> bool {
        self.semantic_id.is_none()
            && self.boost.is_none()
            && self.hidden.is_none()
            && self.metadata_merge.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PatchResponse {
    #[serde(flatten)]
    pub detail: PageDetail,
    /// Whether the title change was propagated into the OpenSearch chunks.
    pub index_synced: bool,
    /// Which path applied boost/hidden: `"onyx_api"` when proxied (Onyx then
    /// syncs its own index), `"direct_sql"` when no Onyx key is configured,
    /// `null` when neither field was touched.
    pub boost_hidden_via: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteOutcome {
    pub pg_deleted: bool,
    pub chunks_deleted: u64,
    /// `true` when Postgres committed but the index delete could not be
    /// confirmed. The document id is queued in `ovis.pending_index_deletes` and
    /// a background task retries — no silent permanent orphans.
    pub index_cleanup_pending: bool,
    pub recrawl_risk: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BatchDeleteRequest {
    pub document_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatchDeleteFailure {
    pub id: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatchDeleteResponse {
    /// True only when `failed` is empty.
    pub success: bool,
    pub deleted: usize,
    pub chunks_deleted: u64,
    pub failed: Vec<BatchDeleteFailure>,
    pub index_cleanup_pending: usize,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Keyword,
    Semantic,
    Hybrid,
}

impl SearchMode {
    pub fn needs_embedding(self) -> bool {
        matches!(self, SearchMode::Semantic | SearchMode::Hybrid)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SearchMode::Keyword => "keyword",
            SearchMode::Semantic => "semantic",
            SearchMode::Hybrid => "hybrid",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub document_id: String,
    pub semantic_id: Option<String>,
    pub link: Option<String>,
    pub score: f64,
    /// Highlighted fragment, with `<em>` around matches. Falls back to the
    /// chunk blurb when the backing query produced no highlight (semantic mode).
    pub snippet: Option<String>,
    pub chunk_index: Option<i64>,
    pub connector_id: Option<i32>,
    pub connector_name: Option<String>,
    pub connector_source: Option<String>,
    pub chunk_count: Option<i32>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResponse {
    pub items: Vec<SearchHit>,
    pub mode: String,
    /// `"no_embedder"` when a semantic/hybrid request fell back to BM25 because
    /// the embedding endpoint is unset or unreachable. `null` otherwise.
    pub degraded: Option<String>,
    pub total_hits: i64,
    pub total_hits_exact: bool,
    pub took_ms: u64,
}

// ---------------------------------------------------------------------------
// Connectors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LastAttempt {
    pub id: Option<i32>,
    pub status: Option<String>,
    pub time_updated: Option<DateTime<Utc>>,
    pub error_msg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorSummary {
    pub connector_id: i32,
    pub cc_pair_id: i32,
    pub name: String,
    pub source: String,
    /// Real `connector_credential_pair.status`: ACTIVE | PAUSED |
    /// INITIAL_INDEXING | DELETING | INVALID. The old code hardcoded
    /// `disabled = false` and never read this.
    pub status: String,
    /// Latest attempt error carries a resilience-cron park sentinel.
    pub parked: bool,
    pub in_repeated_error_state: bool,
    /// Counted from `document_by_connector_credential_pair`.
    /// `connector_credential_pair.total_docs_indexed` is unreliable (often 0)
    /// and is never used.
    pub doc_count: i64,
    pub last_successful_index_time: Option<DateTime<Utc>>,
    pub refresh_freq_secs: Option<i32>,
    pub indexing_trigger: Option<String>,
    pub last_attempt: Option<LastAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttemptAggregates {
    pub success: i64,
    pub failed: i64,
    pub canceled: i64,
    pub in_progress: i64,
    pub not_started: i64,
    pub completed_with_errors: i64,
    pub other: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryPoint {
    pub day: String,
    pub docs_added: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorDetail {
    #[serde(flatten)]
    pub summary: ConnectorSummary,
    /// Verbatim `connector.connector_specific_config`. 58 web connectors point
    /// at the CF-bypass proxy on infra:8765; rendered as-is.
    pub connector_specific_config: Option<serde_json::Value>,
    pub input_type: Option<String>,
    pub prune_freq_secs: Option<i32>,
    pub access_type: Option<String>,
    pub credential_id: Option<i32>,
    /// Credential display name only. Secrets (`credential_json`) are never read.
    pub credential_name: Option<String>,
    pub time_created: Option<DateTime<Utc>>,
    pub time_updated: Option<DateTime<Utc>>,
    pub last_pruned: Option<DateTime<Utc>>,
    pub attempts: AttemptAggregates,
    /// Present only when `?history=<n>d` was requested.
    pub history: Option<Vec<HistoryPoint>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexAttemptItem {
    pub id: i32,
    pub cc_pair_id: i32,
    pub connector_id: Option<i32>,
    pub connector_name: Option<String>,
    pub status: String,
    pub new_docs_indexed: Option<i32>,
    pub total_docs_indexed: Option<i32>,
    pub docs_removed_from_index: Option<i32>,
    pub total_chunks: i32,
    pub completed_batches: i32,
    pub total_batches: Option<i32>,
    pub total_failures_batch_level: i32,
    pub time_created: DateTime<Utc>,
    pub time_started: Option<DateTime<Utc>>,
    pub time_updated: DateTime<Utc>,
    pub error_msg: Option<String>,
    pub from_beginning: bool,
    pub poll_range_start: Option<DateTime<Utc>>,
    pub poll_range_end: Option<DateTime<Utc>>,
    pub last_heartbeat_time: Option<DateTime<Utc>>,
    pub heartbeat_counter: i32,
    pub cancellation_requested: bool,
    pub search_settings_id: Option<i32>,
    /// IN_PROGRESS with no heartbeat/update for 45 min — the same heuristic the
    /// resilience cron uses. Derived from `time_updated`, never from doc counts:
    /// a healthy connector can legitimately sit at 0 docs for a long time.
    pub stalled: bool,
    /// Documents per minute for a running attempt, `null` otherwise.
    pub pages_per_min: Option<f64>,
    /// Latest error carries a park sentinel.
    pub parked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexAttemptError {
    pub id: i32,
    pub index_attempt_id: i32,
    pub cc_pair_id: i32,
    pub document_id: Option<String>,
    pub document_link: Option<String>,
    pub failure_message: String,
    pub error_type: Option<String>,
    pub time_created: DateTime<Utc>,
    pub is_resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexAttemptErrorsResponse {
    pub items: Vec<IndexAttemptError>,
    pub total: i64,
    pub total_exact: bool,
    pub page: Option<i64>,
    pub limit: i64,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    /// `index_attempt_errors` is pruned after 24 h by the resilience cron, so
    /// this is a rolling window, not a full history. Stated in the response so
    /// no client mistakes an empty list for "no failures ever".
    pub window: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundErrorItem {
    pub id: i32,
    pub message: String,
    pub time_created: DateTime<Utc>,
    pub cc_pair_id: Option<i32>,
}

// ---------------------------------------------------------------------------
// Connector actions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunOnceRequest {
    #[serde(default)]
    pub from_beginning: bool,
    /// Required (and must be `true`) when the cc-pair is parked, otherwise the
    /// request is refused with 409 `PARKED_CONNECTOR`.
    #[serde(default)]
    pub acknowledge_parked: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConnectorPatchRequest {
    pub name: Option<String>,
    pub refresh_freq_secs: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConnectorDeleteRequest {
    /// Must match the cc-pair name exactly. Guard against deleting a
    /// 100k-document connector by fat-fingering an id.
    pub confirm_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionResponse {
    pub ok: bool,
    pub cc_pair_id: i32,
    pub action: String,
    /// What the state is *after* the action, when the action reports one.
    pub status: Option<String>,
    /// Verbatim message from Onyx, when it sent one.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TargetedReindexRequest {
    pub cc_pair_id: i32,
    #[serde(default)]
    pub document_ids: Option<Vec<String>>,
    #[serde(default)]
    pub only_failed: Option<bool>,
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagFacet {
    pub key: String,
    pub value: String,
    pub doc_count: i64,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorStatusCounts {
    pub total: i64,
    pub active: i64,
    pub paused: i64,
    pub initial_indexing: i64,
    pub deleting: i64,
    pub invalid: i64,
    pub parked: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexStats {
    pub name: String,
    pub size_bytes: Option<i64>,
    pub docs: Option<i64>,
    pub deleted_docs: Option<i64>,
    pub disk_used_pct: Option<f64>,
    pub disk_total_bytes: Option<i64>,
    pub disk_available_bytes: Option<i64>,
    /// `index.blocks.read_only_allow_delete` — set by OpenSearch when the disk
    /// flood-stage watermark trips. This deployment has hit it before, so it is
    /// a first-class field rather than a footnote.
    pub read_only: bool,
    pub cluster_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingInfo {
    pub model: String,
    pub dim: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrawlStats {
    pub docs_last_15m: i64,
    pub docs_last_24h: i64,
    pub attempts_in_progress: i64,
    pub attempts_stalled: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatsOverview {
    pub documents: i64,
    pub documents_exact: bool,
    pub chunks: Option<i64>,
    pub connectors: ConnectorStatusCounts,
    pub index: IndexStats,
    pub embedding: EmbeddingInfo,
    pub crawl: CrawlStats,
    pub attempts: AttemptAggregates,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimelineBucket {
    pub bucket: DateTime<Utc>,
    pub docs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimelineResponse {
    pub window: String,
    pub bucket: String,
    pub items: Vec<TimelineBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceStat {
    pub source: String,
    pub connectors: i64,
    pub documents: i64,
    pub chunks: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopConnector {
    pub cc_pair_id: i32,
    pub connector_id: i32,
    pub name: String,
    pub source: String,
    pub status: String,
    pub doc_count: i64,
    pub last_successful_index_time: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyHealth {
    pub status: String,
    pub latency_ms: Option<f64>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OnyxHealth {
    pub configured: bool,
    pub status: String,
    pub latency_ms: Option<f64>,
    pub version: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthResponse {
    pub status: String,
    pub postgres: DependencyHealth,
    pub opensearch: DependencyHealth,
    pub onyx_api: OnyxHealth,
    pub embedder: DependencyHealth,
    pub schema_ok: bool,
    pub missing_columns: Vec<String>,
    pub unhandled_document_fk_children: Vec<String>,
    /// OVIS support indexes from `ops/onyx_indexes.sql` that are absent. A
    /// performance warning, never an error — the server works without them.
    pub missing_indexes: Vec<String>,
    pub index_name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeResponse {
    pub index_name: String,
    pub embedding_model: String,
    pub embedding_dim: u32,
    pub query_prefix: String,
    pub search_settings_id: i32,
    pub schema_ok: bool,
    pub refreshed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VersionResponse {
    pub version: String,
    pub git_sha: String,
    pub rustc: String,
    pub built_at: String,
    pub profile: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_patch_rejects_unknown_fields() {
        // A typo like `hiden` must fail loudly rather than silently no-op.
        let err = serde_json::from_str::<PagePatch>(r#"{"hiden": true}"#).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn page_patch_empty_detection() {
        assert!(PagePatch::default().is_empty());
        assert!(!PagePatch {
            hidden: Some(true),
            ..Default::default()
        }
        .is_empty());
    }

    #[test]
    fn page_detail_flattens_the_list_item_fields() {
        let detail = PageDetail {
            item: PageListItem {
                id: "https://example.com/a".into(),
                semantic_id: "A".into(),
                link: None,
                updated_at: "2026-01-01T00:00:00Z".parse().unwrap(),
                doc_updated_at: None,
                last_modified: "2026-01-01T00:00:00Z".parse().unwrap(),
                chunk_count: Some(3),
                boost: 0,
                hidden: false,
                connector_id: Some(1),
                connector_name: Some("c".into()),
                connector_source: Some("WEB".into()),
                metadata: None,
            },
            primary_owners: None,
            secondary_owners: None,
            content_hash: None,
            from_ingestion_api: Some(false),
            last_synced: None,
            cc_pair_id: Some(1),
            cc_pair_status: Some("ACTIVE".into()),
            tags: vec![],
            pg_row: true,
            recrawl_risk: true,
        };
        let v = serde_json::to_value(&detail).unwrap();
        // Flatten means no nested "item" object.
        assert!(v.get("item").is_none());
        assert_eq!(v["id"], "https://example.com/a");
        assert_eq!(v["chunk_count"], 3);
        assert_eq!(v["recrawl_risk"], true);
    }

    #[test]
    fn search_mode_defaults_to_keyword() {
        assert_eq!(SearchMode::default(), SearchMode::Keyword);
        assert!(!SearchMode::Keyword.needs_embedding());
        assert!(SearchMode::Hybrid.needs_embedding());
        assert!(SearchMode::Semantic.needs_embedding());
    }
}
