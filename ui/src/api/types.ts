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
// Pruning (mirrors ovis-core::api_types)
// ---------------------------------------------------------------------------

export type PruneState =
  | 'candidate'
  | 'staged'
  | 'deleting'
  | 'deleted'
  | 'dismissed'
  | 'restored';

/** Reason `detector` vocabulary — what candidate filtering matches. */
export type PruneReasonDetector =
  | 'duplicate'
  | 'language'
  | 'url_rule'
  | 'tag_rule'
  | 'thin'
  | 'stale'
  | 'recrawl'
  | 'quality'
  | 'url_junk'
  | 'policy';

export interface PruneReason {
  detector: PruneReasonDetector;
  code: string;
  detail: string;
  confidence: number;
  evidence: Record<string, unknown>;
}

export interface PruneCandidateItem {
  id: number;
  document_id: string;
  scan_id: number | null;
  state: PruneState;
  reasons: PruneReason[];
  confidence: number;
  recrawl_risk: boolean;
  connector_id: number | null;
  connector_name: string | null;
  cc_pair_id: number | null;
  /** null = "not counted yet" — never render as 0. */
  chunk_count: number | null;
  semantic_id: string | null;
  link: string | null;
  doc_exists: boolean;
  hidden: boolean | null;
  prev_hidden: boolean | null;
  staged_at: string | null;
  stage_expires_at: string | null;
  staged_by: string | null;
  remember: boolean;
  deleted_at: string | null;
  delete_outcome: Record<string, unknown> | null;
  resolved_reason: string | null;
  created_at: string;
  updated_at: string;
}

export interface PrunePairEvidence {
  kept_id: string;
  kept: PageListItem | null;
  similarity: number;
}

export interface PruneCandidateDetail extends PruneCandidateItem {
  pair: PrunePairEvidence | null;
  excluded: boolean;
}

export interface PruneScope {
  kind: 'all' | 'connectors' | 'url_prefix';
  connector_ids?: number[] | null;
  url_prefix?: string | null;
}

/** Scan-launch detector names (distinct from the reason vocabulary). */
/** Mirrors `services::prune::KNOWN_DETECTORS`. */
export type PruneScanDetector =
  | 'exact_duplicate'
  | 'near_duplicate'
  | 'language'
  | 'url_rule'
  | 'tag_rule'
  | 'thin'
  | 'stale'
  | 'quality'
  | 'url_junk'
  | 'url_variant';

export interface PruneScanRequest {
  scope: PruneScope;
  detectors: PruneScanDetector[];
  config_overrides?: Record<string, unknown>;
}

export interface PruneScanItem {
  id: number;
  scope: PruneScope;
  detectors: string[];
  status: 'queued' | 'running' | 'done' | 'failed' | 'cancelled';
  examined: number;
  total: number | null;
  config_hash: string;
  stats: Record<string, number>;
  started_at: string | null;
  finished_at: string | null;
  error: string | null;
  created_at: string;
}

export interface PruneCandidateFilterBody {
  state?: string;
  detector?: string;
  connector_id?: number;
  min_confidence?: number;
  recrawl_risk?: boolean;
  scan_id?: number;
}

export interface PruneSelector {
  ids?: number[];
  filter?: PruneCandidateFilterBody;
}

export interface PruneBulkFailure {
  candidate_id: number;
  document_id: string;
  code: string;
}

export interface PruneBulkResponse {
  success: boolean;
  requested: number;
  changed: number;
  failed: PruneBulkFailure[];
  state: string;
  boost_hidden_via: string | null;
  stage_expires_at: string | null;
}

export interface PruneReaperStatus {
  enabled: boolean;
  next_run_at: string | null;
  last_run_at: string | null;
  halted: boolean;
  halted_reason: string | null;
  deferred: number;
  deferred_reason: string | null;
  deleted_last_hour: number;
}

export interface PruneLimits {
  grace_days: number;
  big_batch: number;
  reaper_batch_size: number;
  max_docs_per_hour: number;
  reaper_interval_secs: number;
}

export interface PruneStatusResponse {
  candidates: number;
  staged: number;
  deleting: number;
  deleted_7d: number;
  deleted_total: number;
  dismissed_total: number;
  restored_total: number;
  exclusions: number;
  soonest_expiry: string | null;
  staged_expiring_24h: number;
  reaper: PruneReaperStatus;
  active_scan: PruneScanItem | null;
  limits: PruneLimits;
  trash: TrashCounts;
}

export interface PruneRuleItem {
  id: number;
  name: string;
  kind: 'url_rule' | 'tag_rule' | 'detector_config';
  body: Record<string, unknown>;
  enabled: boolean;
  updated_at: string;
}

export interface PruneRulePreviewMatch {
  document_id: string;
  semantic_id: string | null;
  matched_on: string;
}

export interface PruneRulePreviewResponse {
  matched: number;
  scanned: number;
  complete: boolean;
  sample: PruneRulePreviewMatch[];
}

export interface PruneExclusionItem {
  document_id: string;
  reason: string;
  note: string | null;
  created_at: string;
}

export interface PruneAuditItem {
  id: number;
  at: string;
  actor: string;
  action: string;
  document_id: string | null;
  scan_id: number | null;
  candidate_id: number | null;
  detail: Record<string, unknown> | null;
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

// ---------------------------------------------------------------------------
// Prune v2 — triage, policy, clusters, trash
// ---------------------------------------------------------------------------

/** What a policy decides for one document. */
export type PruneBand = 'auto' | 'review' | 'none';

export interface PruneBundle {
  key: string;
  title: string;
  description: string;
  detector: string | null;
  documents: number;
  chunks: number;
  mean_confidence: number;
  recrawl_risk: number;
  narration?: Narration;
}

export interface PruneConnectorBundle {
  connector_id: number | null;
  connector_name: string | null;
  documents: number;
  chunks: number;
  mean_confidence: number;
}

export interface TrashCounts {
  items: number;
  bytes: number;
  expiring_7d: number;
  on_hold: number;
  restored_total: number;
  soonest_expiry: string | null;
}

export interface PruneOverviewResponse {
  candidates_open: number;
  documents_total: number;
  profiled: number;
  pairs: number;
  bundles: PruneBundle[];
  by_connector: PruneConnectorBundle[];
  trash: TrashCounts;
}

export interface PruneThreshold {
  auto: number | null;
  review: number | null;
}

export interface PruneQualityThreshold {
  review_min_failures: number | null;
  min_families: number;
  auto_min_failures: number | null;
}

export interface PrunePolicy {
  exact_duplicate: PruneBand;
  url_variant: PruneBand;
  asset: PruneBand;
  stub: PruneBand;
  near_duplicate: PruneThreshold;
  semantic: PruneThreshold;
  quality: PruneQualityThreshold;
  off_topic_percentile: number | null;
  exempt_connectors: string[];
  cross_connector_review_only: boolean;
}

export interface PruneSignalCount {
  signal: string;
  band: string;
  count: number;
}

export interface PruneSampleDoc {
  document_id: string;
  semantic_id: string | null;
  chunk_count: number | null;
  signals: string[];
}

export interface PruneSimulateResponse {
  tier: string | null;
  policy: PrunePolicy;
  policy_hash: string;
  profiled: number;
  auto: number;
  review: number;
  untouched: number;
  by_signal: PruneSignalCount[];
  by_connector: { connector_id: number | null; connector_name: string | null; auto: number; review: number }[];
  auto_sample: PruneSampleDoc[];
  review_sample: PruneSampleDoc[];
  /** What these numbers do not cover — shown, never inferred from a zero. */
  caveats: string[];
}

export interface PruneCommitResponse {
  band: string;
  policy_hash: string;
  created: number;
  skipped: number;
  saved_as: string | null;
}

export interface PruneHistogramBucket {
  lower: number;
  upper: number;
  count: number;
}

export interface PruneClusterMember {
  document_id: string;
  semantic_id: string | null;
  link: string | null;
  chunk_count: number | null;
  updated_at: string | null;
  is_keeper: boolean;
  candidate_id: number | null;
}

export interface PruneCluster {
  key: string;
  method: string;
  size: number;
  members: PruneClusterMember[];
  keeper_reason: string;
  narration?: Narration;
}

/**
 * A generated title and summary.
 *
 * Absent, not empty, when nothing has been generated — a card must be able to
 * tell "not narrated" from "narrated, and the model had nothing to say", and
 * the two look identical if this is an empty string.
 */
export interface Narration {
  subject_key: string;
  title: string;
  summary: string;
  model: string;
  generated_at: string;
}

export interface NarrateResponse {
  subject_kind: string;
  eligible: number;
  already_current: number;
  narrated: Narration[];
  failed: { subject_key: string; reason: string }[];
  model: string;
}

export interface PruneSamplePlan {
  population: number;
  sample_size: number;
  max_failures: number;
  confidence: number;
  max_error_rate: number;
  statement: string;
  documents: PruneSampleDoc[];
}

export interface TrashItem {
  document_id: string;
  semantic_id: string | null;
  connector_id: number | null;
  connector_name: string | null;
  chunk_count: number;
  snapshot_bytes: number;
  vectors_included: boolean;
  reasons: PruneReason[] | null;
  policy_hash: string | null;
  deleted_by: string;
  deleted_at: string;
  expires_at: string;
  hold: boolean;
  /** The id exists in Onyx again — restoring would collide. */
  reappeared: boolean;
}

export interface TrashDetail extends TrashItem {
  text: string;
  chunk_previews: string[];
  document: Record<string, unknown>;
  tags: Record<string, unknown>[];
}

export interface TrashRestoreOutcome {
  document_id: string;
  chunks_restored: number;
  tags_restored: number;
  cc_pairs_restored: number;
  skipped_tags: number;
  skipped_cc_pairs: number;
  index_restore_pending: boolean;
}

export interface TrashBulkResponse {
  success: boolean;
  requested: number;
  changed: number;
  failed: { document_id: string; code: string; message: string }[];
  action: string;
  outcomes: TrashRestoreOutcome[];
}

// ---------------------------------------------------------------------------
// LLM providers, models and roles
// ---------------------------------------------------------------------------

export type LlmProviderKind =
  | 'openai_compatible'
  | 'gemini'
  | 'anthropic'
  | 'ollama'
  | 'llamacpp';

export interface LlmProvider {
  id: number;
  name: string;
  kind: LlmProviderKind;
  base_url: string | null;
  /** The NAME of an environment variable — never a key. */
  api_key_ref: string | null;
  enabled: boolean;
  created_at: string;
  /** Whether that environment variable is actually set on the server. */
  key_present: boolean;
  models: number;
  probed: number;
}

/** What a probe measured. Absent when the model has never been probed — which
 *  is different from "probed and found incapable". */
export interface LlmCapabilities {
  enum_enforced: boolean;
  schema_enforced: boolean;
  logprobs: boolean;
  thinking_channel: 'none' | 'suppressed' | 'unsuppressed';
  notes: string[];
  probe_version: number;
  probed_at: string;
}

export interface LlmAdvertised {
  context_tokens: number | null;
  output_tokens: number | null;
  reasoning: boolean | null;
  is_embedding: boolean;
  description: string | null;
}

export interface LlmModel {
  provider_id: number;
  provider_name: string;
  provider_kind: LlmProviderKind;
  model_id: string;
  display_name: string | null;
  advertised: LlmAdvertised | null;
  capabilities: LlmCapabilities | null;
  probed_at: string | null;
  probe_version: number | null;
  /** Every role this model holds; a model may hold several. */
  roles: string[];
}

export interface LlmProbeResult {
  provider_id: number;
  model_id: string;
  capabilities: LlmCapabilities;
  summary: string;
  usable_as_judge: boolean;
  calibratable: boolean;
}

export type LlmRole = 'bulk' | 'quality' | 'narrate';

export interface LlmRoleAssignment {
  provider_id: number;
  provider_name: string;
  model_id: string;
  display_name: string | null;
  capabilities: LlmCapabilities | null;
}

export type LlmRoles = Record<LlmRole, LlmRoleAssignment | null>;

/**
 * A saved threshold set. `active` marks the deployment's standing answer to
 * "what counts as prunable here".
 */
export interface PruneStoredPolicy {
  id: number;
  name: string;
  tier: string;
  body: PrunePolicy;
  config_hash: string;
  active: boolean;
  created_at: string;
  updated_at: string;
}

/**
 * An acceptance-sampling draw and the claim accepting it would support.
 *
 * `statement` is the server's own words: the arithmetic is a confidence bound,
 * and the decision it feeds is a human's, so the sentence travels with the
 * numbers rather than being rebuilt in the client.
 */
export interface PruneSamplePlan {
  population: number;
  sample_size: number;
  max_failures: number;
  confidence: number;
  max_error_rate: number;
  statement: string;
  documents: PruneSampleDoc[];
}

export interface TagKeyItem {
  key: string;
  distinct_values: number;
}
