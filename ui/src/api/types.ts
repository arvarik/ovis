/**
 * Wire types — a faithful TypeScript mirror of `crates/ovis-core/src/api_types.rs`.
 *
 * Nullability is load-bearing and matches the Rust `Option<T>`s exactly:
 * the backend serialises optional fields as `null` rather than omitting them,
 * so every nullable field here is `T | null`, not `T | undefined`.
 *
 * Notable honest fields (render them, never smooth them over):
 * - `chunk_count: null` means "Onyx has not counted this yet" — NOT zero.
 * - `total_exact: false` means the total is a planner estimate.
 * - `pg_row: false` means orphaned chunks (no Postgres row).
 * - `recrawl_risk: true` means a delete will likely be undone by a recrawl.
 * - `degraded` on search is an OPEN string — render whatever it says.
 */

// ---------------------------------------------------------------------------
// Pagination envelope
// ---------------------------------------------------------------------------

export interface ListResponse<T> {
  items: T[];
  /** Total rows matching the filter. See `total_exact`. */
  total: number;
  /** `false` when `total` is a planner estimate rather than a counted value. */
  total_exact: boolean;
  /** 1-based page for offset pagination; `null` when the request used a cursor. */
  page: number | null;
  limit: number;
  /** Opaque keyset token for the next page. `null` when there is no next page. */
  next_cursor: string | null;
  has_more: boolean;
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

export interface PageListItem {
  /** The document id, which for web-crawled content *is* the URL. */
  id: string;
  /** Title, as Onyx derived it. */
  semantic_id: string;
  link: string | null;
  /** Effective recency: never null; exactly what `sort=updated_*` orders by. */
  updated_at: string;
  /** Onyx's crawl-reported timestamp. Null for ~all rows in this deployment. */
  doc_updated_at: string | null;
  last_modified: string;
  /** `null` = Onyx has not counted yet — NOT the same as 0. */
  chunk_count: number | null;
  boost: number;
  hidden: boolean;
  connector_id: number | null;
  connector_name: string | null;
  /** Upper-case as stored (`WEB`, `GITHUB`, …). */
  connector_source: string | null;
  metadata: Record<string, unknown> | null;
}

export interface TagKv {
  key: string;
  value: string;
}

export interface PageDetail extends PageListItem {
  primary_owners: string[] | null;
  secondary_owners: string[] | null;
  content_hash: string | null;
  from_ingestion_api: boolean | null;
  last_synced: string | null;
  cc_pair_id: number | null;
  cc_pair_status: string | null;
  tags: TagKv[];
  /** `false` ⇒ orphaned chunks: the index has content, Postgres has no row. */
  pg_row: boolean;
  /** `true` ⇒ owning cc-pair is ACTIVE/INITIAL_INDEXING; a delete may be undone. */
  recrawl_risk: boolean;
}

export interface ChunkItem {
  chunk_index: number;
  content: string | null;
  blurb: string | null;
  title: string | null;
  semantic_identifier: string | null;
  source_type: string | null;
  /** Word-count heuristic, honestly named — not a tokeniser result. */
  token_estimate: number | null;
  source_links: Record<string, unknown> | null;
  last_updated: string | null;
  hidden: boolean | null;
  metadata_list: string[] | null;
}

export interface ChunksResponse {
  items: ChunkItem[];
  /** Total chunks the index holds for this document, not `items.length`. */
  total_chunks: number;
  /** `search_after` value for the next page, or `null` at the end. */
  next_after: number | null;
  embedding_model: string;
  embedding_dim: number;
}

export interface ChunkVector {
  dim: number;
  model: string;
  vector: number[];
}

/** Body of `PATCH /pages/{id}`. Absent means unchanged. */
export interface PagePatch {
  semantic_id?: string;
  boost?: number;
  hidden?: boolean;
  /** Shallow-merged into `doc_metadata` — never replaces the whole object. */
  metadata_merge?: Record<string, unknown>;
}

export interface PatchResponse extends PageDetail {
  /** Whether the title change was propagated into the OpenSearch chunks. */
  index_synced: boolean;
  /** `"onyx_api"` | `"direct_sql"` | null (neither boost nor hidden touched). */
  boost_hidden_via: string | null;
}

export interface DeleteOutcome {
  pg_deleted: boolean;
  chunks_deleted: number;
  /** `true`: Postgres committed but the index delete could not be confirmed. */
  index_cleanup_pending: boolean;
  recrawl_risk: boolean;
}

export interface BatchDeleteRequest {
  document_ids: string[];
}

export interface BatchDeleteFailure {
  id: string;
  code: string;
}

export interface BatchDeleteResponse {
  /** True only when `failed` is empty. */
  success: boolean;
  deleted: number;
  chunks_deleted: number;
  failed: BatchDeleteFailure[];
  index_cleanup_pending: number;
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

export type SearchMode = 'keyword' | 'semantic' | 'hybrid';

export interface SearchHit {
  document_id: string;
  semantic_id: string | null;
  link: string | null;
  score: number;
  /** Highlighted fragment with `<em>` around matches; blurb fallback. */
  snippet: string | null;
  chunk_index: number | null;
  connector_id: number | null;
  connector_name: string | null;
  connector_source: string | null;
  chunk_count: number | null;
  updated_at: string | null;
}

export interface SearchResponse {
  items: SearchHit[];
  /** Echoes the REQUESTED mode — a degraded hybrid still says "hybrid". */
  mode: string;
  /**
   * Open string; key off this, never off `mode`. Known values today:
   * `no_knn_field`, `no_embedder`, `connector_filter_post_applied` — but
   * render whatever arrives.
   */
  degraded: string | null;
  total_hits: number;
  total_hits_exact: boolean;
  took_ms: number;
}

// ---------------------------------------------------------------------------
// Connectors
// ---------------------------------------------------------------------------

export interface LastAttempt {
  id: number | null;
  status: string | null;
  time_updated: string | null;
  error_msg: string | null;
}

export interface ConnectorSummary {
  connector_id: number;
  cc_pair_id: number;
  name: string;
  source: string;
  /** ACTIVE | PAUSED | INITIAL_INDEXING | DELETING | INVALID. */
  status: string;
  /** Latest attempt error carries a resilience-cron park sentinel. */
  parked: boolean;
  in_repeated_error_state: boolean;
  /** Counted from `document_by_connector_credential_pair` — the honest count. */
  doc_count: number;
  last_successful_index_time: string | null;
  refresh_freq_secs: number | null;
  indexing_trigger: string | null;
  last_attempt: LastAttempt | null;
}

export interface AttemptAggregates {
  success: number;
  failed: number;
  canceled: number;
  in_progress: number;
  not_started: number;
  completed_with_errors: number;
  other: number;
}

export interface HistoryPoint {
  day: string;
  docs_added: number;
}

export interface ConnectorDetail extends ConnectorSummary {
  connector_specific_config: Record<string, unknown> | null;
  input_type: string | null;
  prune_freq_secs: number | null;
  access_type: string | null;
  credential_id: number | null;
  /** Display name only — secrets are never read. */
  credential_name: string | null;
  time_created: string | null;
  time_updated: string | null;
  last_pruned: string | null;
  attempts: AttemptAggregates;
  /** Present only when `?history=<n>d` was requested. */
  history: HistoryPoint[] | null;
}

export interface IndexAttemptItem {
  id: number;
  cc_pair_id: number;
  connector_id: number | null;
  connector_name: string | null;
  status: string;
  new_docs_indexed: number | null;
  total_docs_indexed: number | null;
  docs_removed_from_index: number | null;
  total_chunks: number;
  completed_batches: number;
  total_batches: number | null;
  total_failures_batch_level: number;
  time_created: string;
  time_started: string | null;
  time_updated: string;
  error_msg: string | null;
  from_beginning: boolean;
  poll_range_start: string | null;
  poll_range_end: string | null;
  last_heartbeat_time: string | null;
  heartbeat_counter: number;
  cancellation_requested: boolean;
  search_settings_id: number | null;
  /** IN_PROGRESS with no heartbeat for 45 min. Never derived from doc counts. */
  stalled: boolean;
  /** Docs per minute for a running attempt, `null` otherwise. */
  pages_per_min: number | null;
  parked: boolean;
}

export interface IndexAttemptError {
  id: number;
  index_attempt_id: number;
  cc_pair_id: number;
  document_id: string | null;
  document_link: string | null;
  failure_message: string;
  error_type: string | null;
  time_created: string;
  is_resolved: boolean;
}

export interface IndexAttemptErrorsResponse extends ListResponse<IndexAttemptError> {
  /** Rolling window ("24h") — an empty list is not "no failures ever". */
  window: string;
}

export interface BackgroundErrorItem {
  id: number;
  message: string;
  time_created: string;
  cc_pair_id: number | null;
}

// ---------------------------------------------------------------------------
// Connector actions
// ---------------------------------------------------------------------------

export interface RunOnceRequest {
  from_beginning?: boolean;
  /** Required true when the cc-pair is parked, else 409 PARKED_CONNECTOR. */
  acknowledge_parked?: boolean;
}

export interface ConnectorPatchRequest {
  name?: string;
  refresh_freq_secs?: number;
}

export interface ConnectorDeleteRequest {
  /** Must match the cc-pair name exactly. */
  confirm_name: string;
}

export interface ActionResponse {
  ok: boolean;
  cc_pair_id: number;
  action: string;
  status: string | null;
  detail: string | null;
}

export interface TargetedReindexRequest {
  cc_pair_id: number;
  document_ids?: string[] | null;
  only_failed?: boolean | null;
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

export interface TagFacet {
  key: string;
  value: string;
  doc_count: number;
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

export interface ConnectorStatusCounts {
  total: number;
  active: number;
  paused: number;
  initial_indexing: number;
  deleting: number;
  invalid: number;
  parked: number;
}

export interface IndexStats {
  name: string;
  size_bytes: number | null;
  docs: number | null;
  deleted_docs: number | null;
  disk_used_pct: number | null;
  disk_total_bytes: number | null;
  disk_available_bytes: number | null;
  /** Flood-stage watermark tripped — first-class; this deployment has hit it. */
  read_only: boolean;
  cluster_status: string | null;
}

export interface EmbeddingInfo {
  model: string;
  dim: number;
}

export interface CrawlStats {
  docs_last_15m: number;
  docs_last_24h: number;
  attempts_in_progress: number;
  attempts_stalled: number;
}

export interface StatsOverview {
  documents: number;
  documents_exact: boolean;
  chunks: number | null;
  connectors: ConnectorStatusCounts;
  index: IndexStats;
  embedding: EmbeddingInfo;
  crawl: CrawlStats;
  attempts: AttemptAggregates;
}

export interface TimelineBucket {
  bucket: string;
  docs: number;
}

export interface TimelineResponse {
  window: string;
  bucket: string;
  items: TimelineBucket[];
}

export interface SourceStat {
  source: string;
  connectors: number;
  documents: number;
  chunks: number | null;
}

export interface TopConnector {
  cc_pair_id: number;
  connector_id: number;
  name: string;
  source: string;
  status: string;
  doc_count: number;
  last_successful_index_time: string | null;
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

export interface DependencyHealth {
  status: string;
  latency_ms: number | null;
  detail: string | null;
}

export interface OnyxHealth {
  configured: boolean;
  status: string;
  latency_ms: number | null;
  /** The Onyx version lives HERE, not on /system/runtime. */
  version: string | null;
  detail: string | null;
}

export interface HealthResponse {
  status: string;
  postgres: DependencyHealth;
  opensearch: DependencyHealth;
  onyx_api: OnyxHealth;
  embedder: DependencyHealth;
  schema_ok: boolean;
  missing_columns: string[];
  unhandled_document_fk_children: string[];
  missing_indexes: string[];
  index_name: string;
  version: string;
}

export interface RuntimeResponse {
  index_name: string;
  embedding_model: string;
  embedding_dim: number;
  query_prefix: string;
  search_settings_id: number;
  schema_ok: boolean;
  refreshed_at: string;
}

export interface VersionResponse {
  version: string;
  git_sha: string;
  rustc: string;
  built_at: string;
  profile: string;
}

// ---------------------------------------------------------------------------
// Error envelope (every non-2xx)
// ---------------------------------------------------------------------------

export interface ApiErrorBody {
  error: {
    code: string;
    message: string;
    status: number;
    req_id: string;
  };
}
