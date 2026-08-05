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

// ---------------------------------------------------------------------------
// Pruning
// ---------------------------------------------------------------------------

/// One typed reason a detector flagged a document. Reasons are shown
/// individually — a candidate's `confidence` is the max reason confidence,
/// never an average.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneReason {
    /// `duplicate | language | url_rule | tag_rule | thin | stale | recrawl | custom`
    pub detector: String,
    /// Machine-stable code, e.g. `exact_duplicate_of`, `chunkless_stub`,
    /// `lang_not_allowed`, `recrawled_after_prune`.
    pub code: String,
    /// Human-specific detail, e.g. "94% similar to https://…/a".
    pub detail: String,
    /// 0.0–1.0. What the number means is defined per detector.
    pub confidence: f32,
    /// Structured evidence: pair ids, similarity, detected language, pattern.
    pub evidence: serde_json::Value,
}

/// One row of `GET /prune/candidates`. Lifecycle state plus enough of the
/// document to review without a second request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneCandidateItem {
    pub id: i64,
    pub document_id: String,
    pub scan_id: Option<i64>,
    /// `candidate | staged | deleting | deleted | dismissed | restored`.
    pub state: String,
    pub reasons: Vec<PruneReason>,
    /// Max reason confidence.
    pub confidence: f32,
    /// Derived from the owning cc-pair status at flag time.
    pub recrawl_risk: bool,
    pub connector_id: Option<i32>,
    pub connector_name: Option<String>,
    pub cc_pair_id: Option<i32>,
    /// Live value when the document row still exists, otherwise the value
    /// recorded at flag time. `null` means "not counted yet", never "empty".
    pub chunk_count: Option<i32>,
    /// Live title/link, `null` after deletion.
    pub semantic_id: Option<String>,
    pub link: Option<String>,
    /// Whether the `document` row still exists.
    pub doc_exists: bool,
    /// The document's current `hidden` flag, when the row exists.
    pub hidden: Option<bool>,
    /// `hidden` as it was immediately before staging — what restore returns to.
    pub prev_hidden: Option<bool>,
    pub staged_at: Option<DateTime<Utc>>,
    /// When the grace period ends and the reaper may delete. Server-side truth.
    pub stage_expires_at: Option<DateTime<Utc>>,
    pub staged_by: Option<String>,
    /// Whether deletion should record an exclusion (auto-restage on recrawl).
    pub remember: bool,
    pub deleted_at: Option<DateTime<Utc>>,
    /// `{ chunks_deleted, index_cleanup_pending }` for deleted rows.
    pub delete_outcome: Option<serde_json::Value>,
    /// Why a terminal state was reached: `restored | dismissed |
    /// no_longer_matches | …`.
    pub resolved_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `GET /prune/candidates/{id}` — the item plus hydrated duplicate-pair
/// evidence when a duplicate reason exists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneCandidateDetail {
    #[serde(flatten)]
    pub item: PruneCandidateItem,
    /// Both sides of the duplicate pair, when a duplicate reason exists.
    pub pair: Option<PrunePairEvidence>,
    /// Whether this document id is on the exclusion list.
    pub excluded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrunePairEvidence {
    /// The keeper's document id (from the reason evidence).
    pub kept_id: String,
    /// The keeper document, when it still exists.
    pub kept: Option<PageListItem>,
    /// Estimated Jaccard (near) or 1.0 (exact).
    pub similarity: f64,
}

/// Scan scope. `kind` is `all | connectors | url_prefix`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneScope {
    pub kind: String,
    #[serde(default)]
    pub connector_ids: Option<Vec<i32>>,
    #[serde(default)]
    pub url_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneScanRequest {
    pub scope: PruneScope,
    /// Detector names to run: `exact_duplicate | near_duplicate | language |
    /// url_rule | tag_rule | thin | stale`. Explicit — nothing runs unasked.
    pub detectors: Vec<String>,
    /// Optional per-scan overrides, merged over stored detector config.
    #[serde(default)]
    pub config_overrides: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneScanItem {
    pub id: i64,
    pub scope: PruneScope,
    pub detectors: Vec<String>,
    /// `queued | running | done | failed | cancelled`.
    pub status: String,
    /// Documents examined so far. Live while running.
    pub examined: i64,
    /// Total documents in scope, when known.
    pub total: Option<i64>,
    /// Hash of the effective detector config this scan ran under.
    pub config_hash: String,
    /// `{ candidates_new, candidates_updated, candidates_closed,
    ///    excluded_skipped, … }` — counters, all server truths.
    pub stats: serde_json::Value,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Filter half of a bulk selector. Mirrors the `GET /prune/candidates` filters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneCandidateFilterBody {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub detector: Option<String>,
    #[serde(default)]
    pub connector_id: Option<i32>,
    #[serde(default)]
    pub min_confidence: Option<f32>,
    #[serde(default)]
    pub recrawl_risk: Option<bool>,
    #[serde(default)]
    pub scan_id: Option<i64>,
}

/// `POST /prune/candidates/stage` — candidate → staged (hidden, grace starts).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneStageRequest {
    #[serde(default)]
    pub ids: Option<Vec<i64>>,
    #[serde(default)]
    pub filter: Option<PruneCandidateFilterBody>,
    /// Must equal the resolved selection size, or the request is a 409 carrying
    /// the fresh count. No bulk mutation runs on a drifted set.
    pub confirm_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneDismissRequest {
    #[serde(default)]
    pub ids: Option<Vec<i64>>,
    #[serde(default)]
    pub filter: Option<PruneCandidateFilterBody>,
    /// Also record the documents on the exclusion list so future scans never
    /// re-flag them.
    #[serde(default)]
    pub exclude_future: bool,
    /// Optional here (dismiss keeps data); verified when present.
    #[serde(default)]
    pub confirm_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneRestoreRequest {
    #[serde(default)]
    pub ids: Option<Vec<i64>>,
    #[serde(default)]
    pub filter: Option<PruneCandidateFilterBody>,
    /// Optional: restore is the safe direction. Verified when present.
    #[serde(default)]
    pub confirm_count: Option<i64>,
}

/// `POST /prune/candidates/schedule-delete`. Never deletes inline: candidates
/// are staged first (grace applies in full), already-staged documents have
/// their grace deadline brought forward to now. The reaper executes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneScheduleDeleteRequest {
    #[serde(default)]
    pub ids: Option<Vec<i64>>,
    #[serde(default)]
    pub filter: Option<PruneCandidateFilterBody>,
    pub confirm_count: i64,
    /// Record an exclusion at delete time so a recrawl auto-stages the document
    /// again. `null` defaults to the per-document `recrawl_risk`.
    #[serde(default)]
    pub remember: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneBulkFailure {
    pub candidate_id: i64,
    pub document_id: String,
    pub code: String,
}

/// Outcome of a bulk lifecycle mutation, per-id honest like batch delete.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneBulkResponse {
    /// True only when `failed` is empty.
    pub success: bool,
    /// Selection size after resolution (what `confirm_count` had to match).
    pub requested: i64,
    pub changed: i64,
    pub failed: Vec<PruneBulkFailure>,
    /// Resulting state of the changed rows.
    pub state: String,
    /// `onyx_api` or `direct_sql` for stage/restore, `null` otherwise.
    pub boost_hidden_via: Option<String>,
    /// For staging: when the batch's grace ends (latest deadline in the batch).
    pub stage_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneReaperStatus {
    pub enabled: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    /// Halted means the reaper refuses to delete (index read-only) and says so.
    pub halted: bool,
    pub halted_reason: Option<String>,
    /// Documents whose deletion was deferred last cycle (owning pair indexing).
    pub deferred: i64,
    pub deferred_reason: Option<String>,
    /// Deleted in the trailing hour, against `max_docs_per_hour`.
    pub deleted_last_hour: i64,
}

/// Server-side limits, surfaced so clients can render guardrails honestly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneLimits {
    pub grace_days: i64,
    pub big_batch: i64,
    pub reaper_batch_size: i64,
    pub max_docs_per_hour: i64,
    pub reaper_interval_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneStatusResponse {
    /// Open candidates awaiting review.
    pub candidates: i64,
    pub staged: i64,
    pub deleting: i64,
    pub deleted_7d: i64,
    pub deleted_total: i64,
    pub dismissed_total: i64,
    pub restored_total: i64,
    pub exclusions: i64,
    /// Soonest staged expiry — the countdown the UI shows.
    pub soonest_expiry: Option<DateTime<Utc>>,
    pub staged_expiring_24h: i64,
    pub reaper: PruneReaperStatus,
    /// The currently running or queued scan, if any.
    pub active_scan: Option<PruneScanItem>,
    pub limits: PruneLimits,
    /// Deleted documents still restorable from the trash. Part of status
    /// rather than a separate call because the recovery window belongs next to
    /// the deletion counts, not one tab away from them.
    pub trash: crate::db::trash::TrashCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneRuleItem {
    pub id: i64,
    pub name: String,
    /// `url_rule | tag_rule | detector_config`.
    pub kind: String,
    pub body: serde_json::Value,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneRuleCreate {
    pub name: String,
    pub kind: String,
    pub body: serde_json::Value,
    /// Rules start disabled unless explicitly enabled.
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PruneRulePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneRulePreviewMatch {
    pub document_id: String,
    pub semantic_id: Option<String>,
    /// What the pattern matched against (the URL, or `key=value` for tag rules).
    pub matched_on: String,
}

/// `POST /prune/rules/{id}/preview` — sample matches against live data.
/// Never mutates. `complete: false` means the preview cap stopped the scan
/// early and `matched` is a lower bound over `scanned` rows, not a total.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneRulePreviewResponse {
    pub matched: i64,
    pub scanned: i64,
    pub complete: bool,
    pub sample: Vec<PruneRulePreviewMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneExclusionItem {
    pub document_id: String,
    /// `deleted_with_remember | user_excluded`.
    pub reason: String,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneAuditItem {
    pub id: i64,
    pub at: DateTime<Utc>,
    /// Bearer-authenticated caller, `local`, or a background task name
    /// (`reaper`, `scan`).
    pub actor: String,
    pub action: String,
    pub document_id: Option<String>,
    pub scan_id: Option<i64>,
    pub candidate_id: Option<i64>,
    pub detail: Option<serde_json::Value>,
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
