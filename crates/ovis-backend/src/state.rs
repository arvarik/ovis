//! Shared state: clients, caches, and the runtime metadata that is re-read
//! rather than hardcoded.

use std::sync::Arc;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use moka::future::Cache;
use ovis_core::api_types::{ConnectorSummary, TagFacet};
use ovis_core::db::probe::{SchemaProbe, SearchSettings};
use ovis_core::search::{EmbedClient, IndexCapabilities, OsClient};
use ovis_core::{CoreError, CoreResult};
use sqlx::PgPool;
use std::time::Duration;

use crate::config::ServerConfig;

/// Facts about the deployment that can change under us while the server runs.
///
/// Re-read on a timer so an Onyx re-embed switchover — which creates a second
/// index and moves the `PRESENT` `search_settings` row — is picked up without a
/// restart. The old code hardcoded the `danswer_chunk*` wildcard, which during
/// exactly that window would have fanned deletes across both indexes.
#[derive(Debug, Clone)]
pub struct RuntimeMeta {
    pub index_name: String,
    pub embedding_model: String,
    pub embedding_dim: u32,
    pub query_prefix: String,
    pub search_settings_id: i32,
    /// What the live index can actually serve. See [`IndexCapabilities`] — on
    /// this deployment the declared kNN field holds no documents, so semantic
    /// search degrades rather than silently returning nothing.
    pub capabilities: IndexCapabilities,
    pub schema: SchemaProbe,
    pub refreshed_at: DateTime<Utc>,
}

impl RuntimeMeta {
    /// Load everything from scratch: `search_settings`, the schema probe, and the
    /// index capability probe.
    pub async fn load(pool: &PgPool, os: &OsClient) -> CoreResult<Self> {
        let settings: SearchSettings = ovis_core::db::probe::load_search_settings(pool).await?;
        let schema = ovis_core::db::probe::probe_schema(pool).await?;
        // A capability probe failure must not stop the server: OpenSearch being
        // briefly unreachable should degrade search, not prevent boot.
        let capabilities = match os.probe_capabilities(&settings.index_name).await {
            Ok(caps) => caps,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    index = %settings.index_name,
                    "could not probe index capabilities; semantic search stays disabled \
                     until the next refresh"
                );
                IndexCapabilities::default()
            }
        };

        Ok(Self {
            index_name: settings.index_name,
            embedding_model: settings.model_name,
            embedding_dim: settings.model_dim.max(0) as u32,
            query_prefix: settings.query_prefix,
            search_settings_id: settings.id,
            capabilities,
            schema,
            refreshed_at: Utc::now(),
        })
    }

    /// Whether an endpoint that needs `table.column` can be served.
    pub fn requires_column(&self, table: &str, column: &str) -> CoreResult<()> {
        if self.schema.has_column(table, column) {
            Ok(())
        } else {
            Err(CoreError::SchemaMismatch(format!(
                "this endpoint reads {table}.{column}, which is not present in the Onyx \
                 schema; see /api/v1/system/health for the full list"
            )))
        }
    }

    /// Whether the delete sweep covers every restricting FK child of `document`.
    pub fn delete_is_safe(&self) -> CoreResult<()> {
        if self.schema.unhandled_fk_children.is_empty() {
            Ok(())
        } else {
            Err(CoreError::SchemaMismatch(format!(
                "refusing to delete: {} now reference document(id) and the cascade does not \
                 clear them, so a delete would fail mid-transaction",
                self.schema.unhandled_fk_children.join(", ")
            )))
        }
    }
}

/// TTL caches. Every entry is derived data that can be recomputed; nothing here
/// is authoritative.
#[derive(Clone)]
pub struct Caches {
    /// Exact `count(*)` per filter key. TTL 30 s.
    pub counts: Cache<String, i64>,
    /// Marks an exact-count refresh already running for a filter key, so a burst
    /// of requests triggers one background count rather than one each.
    pub count_inflight: Cache<String, ()>,
    /// The connector summary list. TTL 15 s, invalidated by any action.
    pub connectors: Cache<(), Arc<Vec<ConnectorSummary>>>,
    /// Tag facet lists per query key. TTL 60 s.
    pub facets: Cache<String, Arc<Vec<TagFacet>>>,
    /// Stats payloads per key. TTL 30 s.
    pub stats: Cache<String, Arc<serde_json::Value>>,
}

impl Caches {
    pub fn new() -> Self {
        Self {
            counts: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(30))
                .build(),
            count_inflight: Cache::builder()
                .max_capacity(1_000)
                .time_to_live(Duration::from_secs(60))
                .build(),
            connectors: Cache::builder()
                .max_capacity(1)
                .time_to_live(Duration::from_secs(15))
                .build(),
            facets: Cache::builder()
                .max_capacity(1_000)
                .time_to_live(Duration::from_secs(60))
                .build(),
            stats: Cache::builder()
                .max_capacity(100)
                .time_to_live(Duration::from_secs(30))
                .build(),
        }
    }

    /// Drop everything a document mutation can invalidate.
    ///
    /// Totals and connector counts both move when a document is deleted or
    /// hidden, and a UI that refreshes right after its own action must not be
    /// shown the pre-action numbers.
    pub async fn invalidate_document_scoped(&self) {
        self.counts.invalidate_all();
        self.count_inflight.invalidate_all();
        self.connectors.invalidate_all();
        self.stats.invalidate_all();
    }

    /// Drop everything a connector action can invalidate.
    pub async fn invalidate_connector_scoped(&self) {
        self.connectors.invalidate_all();
        self.stats.invalidate_all();
    }

    /// Entry counts, after settling moka's pending bookkeeping.
    ///
    /// `entry_count()` is eventually consistent: without this it reports 0 for a
    /// cache that has just been written to, which reads as "the cache is not
    /// working" rather than "the counter has not caught up".
    pub async fn entry_counts(&self) -> serde_json::Value {
        self.counts.run_pending_tasks().await;
        self.connectors.run_pending_tasks().await;
        self.facets.run_pending_tasks().await;
        self.stats.run_pending_tasks().await;
        serde_json::json!({
            "counts": self.counts.entry_count(),
            "connectors": self.connectors.entry_count(),
            "facets": self.facets.entry_count(),
            "stats": self.stats.entry_count(),
        })
    }
}

impl Default for Caches {
    fn default() -> Self {
        Self::new()
    }
}

/// Build information, stamped at compile time by `build.rs`.
#[derive(Debug, Clone)]
pub struct BuildInfo {
    pub version: &'static str,
    pub git_sha: &'static str,
    pub rustc: &'static str,
    pub built_at: &'static str,
    pub profile: &'static str,
}

impl BuildInfo {
    pub const fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            git_sha: env!("OVIS_GIT_SHA"),
            rustc: env!("OVIS_RUSTC_VERSION"),
            built_at: env!("OVIS_BUILT_AT"),
            profile: env!("OVIS_PROFILE"),
        }
    }
}

/// Live reaper state. In-memory only — the durable truth (candidate states,
/// audit) lives in `ovis.*`; this is the "what is the reaper doing right now"
/// that `/prune/status` reports.
#[derive(Debug, Clone, Default)]
pub struct PruneReaperState {
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    /// Set while the reaper refuses to delete (e.g. `index_read_only`).
    pub halted_reason: Option<String>,
    /// Documents whose deletion was deferred in the last cycle.
    pub deferred: i64,
    pub deferred_reason: Option<String>,
}

/// Shared pruning runtime: whether the `ovis.prune_*` tables exist, the
/// reaper's live state, and the scan runner's wake-up handle.
#[derive(Clone)]
pub struct PruneHandle {
    /// `false` when the database user could not create the tables; every
    /// `/prune/*` endpoint reports the feature unavailable.
    pub enabled: bool,
    /// `false` when `ovis.trash_document` could not be created.
    ///
    /// The reaper refuses to delete anything while this is false. Deleting
    /// without a place to put the snapshot would be the pre-v2 behaviour —
    /// irreversible — and silently falling back to it is precisely the
    /// failure the trash exists to rule out.
    pub trash_enabled: bool,
    pub reaper: Arc<std::sync::RwLock<PruneReaperState>>,
    /// Notified when a scan is queued, so the runner picks it up immediately
    /// rather than at its next poll tick.
    pub scan_wake: Arc<tokio::sync::Notify>,
}

impl PruneHandle {
    pub fn new(enabled: bool, trash_enabled: bool) -> Self {
        Self {
            enabled,
            trash_enabled,
            reaper: Arc::new(std::sync::RwLock::new(PruneReaperState::default())),
            scan_wake: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn reaper_state(&self) -> PruneReaperState {
        self.reaper.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn update_reaper<F: FnOnce(&mut PruneReaperState)>(&self, f: F) {
        if let Ok(mut guard) = self.reaper.write() {
            f(&mut guard);
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub os: OsClient,
    /// `None` until an Onyx URL *and* key are configured; action routes answer
    /// `503 ONYX_UNCONFIGURED` in that case.
    pub onyx: Option<ovis_core::onyx::OnyxClient>,
    /// `None` disables hybrid/semantic search gracefully.
    pub embed: Option<EmbedClient>,
    pub caches: Caches,
    pub runtime: Arc<ArcSwap<RuntimeMeta>>,
    pub cfg: Arc<ServerConfig>,
    pub build: BuildInfo,
    /// Whether `ovis.pending_index_deletes` exists, i.e. whether a failed index
    /// delete will be retried.
    pub pending_deletes_enabled: bool,
    pub prune: PruneHandle,
    /// `false` when the `ovis.llm_*` tables could not be created; every
    /// `/llm/*` endpoint then reports the feature unavailable rather than
    /// half-working.
    pub llm_enabled: bool,
    pub metrics: Option<Arc<metrics_exporter_prometheus::PrometheusHandle>>,
}

impl AppState {
    pub fn runtime(&self) -> Arc<RuntimeMeta> {
        self.runtime.load_full()
    }

    pub fn index_name(&self) -> String {
        self.runtime().index_name.clone()
    }

    /// The Onyx client, or the error that tells the caller to configure one.
    pub fn onyx(&self) -> Result<&ovis_core::onyx::OnyxClient, crate::error::AppError> {
        self.onyx
            .as_ref()
            .ok_or(crate::error::AppError::OnyxUnconfigured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn document_mutations_invalidate_totals_and_connector_counts() {
        let caches = Caches::new();
        caches.counts.insert("all".into(), 1_652_044).await;
        caches.connectors.insert((), Arc::new(Vec::new())).await;
        caches
            .stats
            .insert("overview".into(), Arc::new(serde_json::json!({})))
            .await;
        caches
            .facets
            .insert("author".into(), Arc::new(Vec::new()))
            .await;
        caches.counts.run_pending_tasks().await;

        assert_eq!(caches.counts.get("all").await, Some(1_652_044));

        caches.invalidate_document_scoped().await;
        caches.counts.run_pending_tasks().await;
        caches.connectors.run_pending_tasks().await;
        caches.stats.run_pending_tasks().await;
        caches.facets.run_pending_tasks().await;

        assert_eq!(caches.counts.get("all").await, None);
        assert_eq!(caches.connectors.get(&()).await, None);
        assert_eq!(caches.stats.get("overview").await, None);
        // Tag facets do not move when a single document changes, so they stay.
        assert!(caches.facets.get("author").await.is_some());
    }

    #[tokio::test]
    async fn connector_actions_leave_document_totals_alone() {
        let caches = Caches::new();
        caches.counts.insert("all".into(), 42).await;
        caches.connectors.insert((), Arc::new(Vec::new())).await;
        caches.counts.run_pending_tasks().await;

        caches.invalidate_connector_scoped().await;
        caches.counts.run_pending_tasks().await;
        caches.connectors.run_pending_tasks().await;

        assert_eq!(caches.counts.get("all").await, Some(42));
        assert_eq!(caches.connectors.get(&()).await, None);
    }

    fn meta_with(schema: SchemaProbe) -> RuntimeMeta {
        RuntimeMeta {
            index_name: "danswer_chunk_snowflake_arctic_embed_m".into(),
            embedding_model: "snowflake-arctic-embed:m".into(),
            embedding_dim: 768,
            query_prefix: String::new(),
            search_settings_id: 4,
            capabilities: IndexCapabilities::default(),
            schema,
            refreshed_at: Utc::now(),
        }
    }

    #[test]
    fn a_missing_column_turns_into_a_schema_mismatch_not_a_wrong_answer() {
        let meta = meta_with(SchemaProbe {
            missing_columns: vec!["document.chunk_count".into()],
            ..Default::default()
        });
        let err = meta.requires_column("document", "chunk_count").unwrap_err();
        assert!(matches!(err, CoreError::SchemaMismatch(_)));
        assert!(err.to_string().contains("document.chunk_count"));
        assert!(meta.requires_column("document", "boost").is_ok());
    }

    #[test]
    fn delete_refuses_when_a_new_fk_child_appears() {
        let meta = meta_with(SchemaProbe {
            unhandled_fk_children: vec!["document_annotation.document_id".into()],
            ..Default::default()
        });
        let err = meta.delete_is_safe().unwrap_err();
        assert!(err.to_string().contains("document_annotation.document_id"));
        assert!(meta_with(SchemaProbe::default()).delete_is_safe().is_ok());
    }

    #[test]
    fn build_info_is_populated_at_compile_time() {
        let build = BuildInfo::current();
        assert!(!build.version.is_empty());
        assert!(!build.rustc.is_empty());
        assert!(!build.built_at.is_empty());
        assert!(!build.git_sha.is_empty());
    }
}
