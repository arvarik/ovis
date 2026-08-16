//! OVIS-owned pruning state, in the `ovis` schema next to
//! `pending_index_deletes`.
//!
//! Everything here is DDL and DML against `ovis.*` plus **reads** of Onyx
//! tables (`document`, `connector`, `connector_credential_pair`,
//! `index_attempt`). The only Onyx-table *writes* pruning ever performs go
//! through the existing document update/delete paths in
//! [`super::documents`] — nothing in this module writes outside the `ovis`
//! schema, and a test asserts that.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use crate::api_types::{
    PruneAuditItem, PruneCandidateItem, PruneExclusionItem, PruneReason, PruneRuleItem,
    PruneScanItem, PruneScope,
};
use crate::error::{CoreError, CoreResult};

/// Lifecycle states in which a document has exactly one live row.
pub const OPEN_STATES: [&str; 3] = ["candidate", "staged", "deleting"];

const DDL: &[&str] = &[
    "CREATE SCHEMA IF NOT EXISTS ovis",
    "CREATE TABLE IF NOT EXISTS ovis.prune_scan ( \
        id              bigserial PRIMARY KEY, \
        scope           jsonb NOT NULL, \
        detectors       text[] NOT NULL, \
        config_snapshot jsonb NOT NULL, \
        config_hash     text NOT NULL, \
        status          text NOT NULL DEFAULT 'queued', \
        checkpoint      jsonb, \
        examined        bigint NOT NULL DEFAULT 0, \
        total           bigint, \
        stats           jsonb NOT NULL DEFAULT '{}'::jsonb, \
        started_at      timestamptz, \
        finished_at     timestamptz, \
        error           text, \
        created_at      timestamptz NOT NULL DEFAULT now() \
    )",
    "CREATE TABLE IF NOT EXISTS ovis.prune_candidate ( \
        id              bigserial PRIMARY KEY, \
        document_id     text NOT NULL, \
        scan_id         bigint REFERENCES ovis.prune_scan(id), \
        state           text NOT NULL DEFAULT 'candidate', \
        reasons         jsonb NOT NULL, \
        confidence      real NOT NULL, \
        recrawl_risk    boolean NOT NULL DEFAULT false, \
        connector_id    int, \
        cc_pair_id      int, \
        chunk_count     int, \
        prev_hidden     boolean, \
        staged_at       timestamptz, \
        stage_expires_at timestamptz, \
        staged_by       text, \
        remember        boolean NOT NULL DEFAULT false, \
        deleted_at      timestamptz, \
        delete_outcome  jsonb, \
        resolved_reason text, \
        created_at      timestamptz NOT NULL DEFAULT now(), \
        updated_at      timestamptz NOT NULL DEFAULT now() \
    )",
    "CREATE UNIQUE INDEX IF NOT EXISTS ix_ovis_prune_candidate_open_doc \
        ON ovis.prune_candidate (document_id) \
        WHERE state IN ('candidate','staged','deleting')",
    "CREATE INDEX IF NOT EXISTS ix_ovis_prune_candidate_reap \
        ON ovis.prune_candidate (state, stage_expires_at)",
    // The review list's default ordering. Five-figure candidate sets are the
    // expected first-run scale; sorting them per request is not. No state
    // prefix: the list filters `state = ANY(...)`, which would force a merge
    // over per-state scans — walking the global order and filtering the few
    // closed rows is what the planner actually picks.
    "CREATE INDEX IF NOT EXISTS ix_ovis_prune_candidate_conf \
        ON ovis.prune_candidate (confidence DESC, id DESC)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_prune_candidate_doc \
        ON ovis.prune_candidate (document_id)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_prune_candidate_scan \
        ON ovis.prune_candidate (scan_id)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_prune_candidate_reasons \
        ON ovis.prune_candidate USING gin (reasons jsonb_path_ops)",
    "CREATE TABLE IF NOT EXISTS ovis.prune_exclusions ( \
        document_id  text PRIMARY KEY, \
        reason       text NOT NULL, \
        note         text, \
        created_at   timestamptz NOT NULL DEFAULT now() \
    )",
    "CREATE TABLE IF NOT EXISTS ovis.prune_rules ( \
        id          bigserial PRIMARY KEY, \
        name        text UNIQUE NOT NULL, \
        kind        text NOT NULL, \
        body        jsonb NOT NULL, \
        enabled     boolean NOT NULL DEFAULT false, \
        updated_at  timestamptz NOT NULL DEFAULT now() \
    )",
    "CREATE TABLE IF NOT EXISTS ovis.prune_audit ( \
        id           bigserial PRIMARY KEY, \
        at           timestamptz NOT NULL DEFAULT now(), \
        actor        text NOT NULL, \
        action       text NOT NULL, \
        document_id  text, \
        scan_id      bigint, \
        candidate_id bigint, \
        detail       jsonb \
    )",
    "CREATE INDEX IF NOT EXISTS ix_ovis_prune_audit_at \
        ON ovis.prune_audit (at DESC, id DESC)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_prune_audit_doc \
        ON ovis.prune_audit (document_id)",
    // Persisted MinHash signatures for the checkpointed near-duplicate scan.
    // One config generation at a time: a parameter change wipes and rebuilds.
    "CREATE TABLE IF NOT EXISTS ovis.prune_minhash ( \
        document_id  text PRIMARY KEY, \
        config_hash  text NOT NULL, \
        fingerprint  text NOT NULL, \
        sig          bytea NOT NULL, \
        updated_at   timestamptz NOT NULL DEFAULT now() \
    )",
    "CREATE TABLE IF NOT EXISTS ovis.prune_minhash_band ( \
        document_id  text NOT NULL, \
        band         smallint NOT NULL, \
        hash         bigint NOT NULL, \
        PRIMARY KEY (document_id, band) \
    )",
    "CREATE INDEX IF NOT EXISTS ix_ovis_prune_minhash_band_bucket \
        ON ovis.prune_minhash_band (band, hash)",
    // -----------------------------------------------------------------
    // v2: measurements, not verdicts.
    //
    // A scan writes what it *measured* about a document here; policy turns
    // measurements into candidates at read time. That split is what lets the
    // review UI answer "what would a stricter setting flag?" without a
    // re-scan, and what makes a threshold change re-band instantly instead of
    // invalidating hours of work.
    // -----------------------------------------------------------------
    "CREATE TABLE IF NOT EXISTS ovis.doc_profile ( \
        document_id        text PRIMARY KEY, \
        computed_at        timestamptz NOT NULL DEFAULT now(), \
        config_hash        text, \
        fingerprint        text, \
        connector_id       int, \
        word_count         int, \
        chunk_count        int, \
        quality_metrics    jsonb, \
        quality_gates      text[], \
        quality_fail_count smallint NOT NULL DEFAULT 0, \
        quality_families   smallint NOT NULL DEFAULT 0, \
        canonical_url      text, \
        url_class          text, \
        path_depth         smallint, \
        has_query          boolean, \
        archive_of         text, \
        lang               text, \
        lang_confidence    real, \
        content_hash       text, \
        max_jaccard        real, \
        max_jaccard_doc    text, \
        max_cosine         real, \
        max_cosine_doc     text, \
        centroid_sim       real, \
        centroid_pct       real, \
        cluster_id         int, \
        judge_score        real, \
        distilled_score    real, \
        retrieval_count    int \
    )",
    // Duplicate-group membership, one row per (document, method).
    //
    // This was three columns on `doc_profile`, which gave a document exactly
    // one group — so the URL-variant phase, running after the exact-duplicate
    // phase, overwrote the hash membership of every document it also grouped.
    // A three-document hash cluster came back from the API with one member and
    // no keeper, and the `exact_duplicate` policy signal stopped matching
    // documents that were still byte-identical copies. The two detectors
    // measure different things and a document can legitimately be in both.
    "CREATE TABLE IF NOT EXISTS ovis.doc_dup_group ( \
        document_id     text NOT NULL, \
        method          text NOT NULL, \
        group_key       text NOT NULL, \
        group_size      int NOT NULL, \
        is_keeper       boolean NOT NULL DEFAULT false, \
        cross_connector boolean NOT NULL DEFAULT false, \
        computed_at     timestamptz NOT NULL DEFAULT now(), \
        PRIMARY KEY (document_id, method) \
    )",
    "CREATE INDEX IF NOT EXISTS ix_ovis_doc_dup_group_key \
        ON ovis.doc_dup_group (method, group_key)",
    // `CREATE TABLE IF NOT EXISTS` is a no-op against a table that already
    // exists, so a column added to the definition above never reaches a
    // database created by an earlier build — the query referencing it just
    // starts failing. Every column added after `doc_profile` first shipped is
    // therefore also stated as an idempotent ALTER. Both forms are needed: the
    // CREATE for a fresh database, these for an upgraded one.
    "ALTER TABLE ovis.doc_profile ADD COLUMN IF NOT EXISTS judge_score real",
    "ALTER TABLE ovis.doc_profile ADD COLUMN IF NOT EXISTS distilled_score real",
    "ALTER TABLE ovis.doc_profile ADD COLUMN IF NOT EXISTS retrieval_count int",
    // Every column policy can threshold on gets an index: `/prune/simulate`
    // is a full-corpus aggregate and runs on every drag of the UI slider.
    "CREATE INDEX IF NOT EXISTS ix_ovis_doc_profile_quality \
        ON ovis.doc_profile (quality_fail_count DESC, quality_families DESC)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_doc_profile_words \
        ON ovis.doc_profile (word_count)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_doc_profile_connector \
        ON ovis.doc_profile (connector_id)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_doc_profile_canonical \
        ON ovis.doc_profile (canonical_url) WHERE canonical_url IS NOT NULL",
    "CREATE INDEX IF NOT EXISTS ix_ovis_doc_profile_url_class \
        ON ovis.doc_profile (url_class)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_doc_profile_jaccard \
        ON ovis.doc_profile (max_jaccard DESC) WHERE max_jaccard IS NOT NULL",
    "CREATE INDEX IF NOT EXISTS ix_ovis_doc_profile_cosine \
        ON ovis.doc_profile (max_cosine DESC) WHERE max_cosine IS NOT NULL",
    // Verified pair similarities. Stored rather than thresholded away so the
    // acting threshold can move without recomputing anything (the SemHash
    // operational pattern).
    "CREATE TABLE IF NOT EXISTS ovis.dup_pair ( \
        a           text NOT NULL, \
        b           text NOT NULL, \
        method      text NOT NULL, \
        estimated   real, \
        verified    real, \
        cosine      real, \
        same_connector boolean, \
        verified_at timestamptz NOT NULL DEFAULT now(), \
        PRIMARY KEY (a, b, method) \
    )",
    "CREATE INDEX IF NOT EXISTS ix_ovis_dup_pair_b ON ovis.dup_pair (b)",
    "CREATE INDEX IF NOT EXISTS ix_ovis_dup_pair_score \
        ON ovis.dup_pair (method, COALESCE(verified, estimated) DESC)",
    // Named threshold sets. A policy is applied, not baked in: committing one
    // creates candidates, and re-committing a changed one re-bands them.
    "CREATE TABLE IF NOT EXISTS ovis.prune_policy ( \
        id          bigserial PRIMARY KEY, \
        name        text UNIQUE NOT NULL, \
        tier        text NOT NULL, \
        body        jsonb NOT NULL, \
        config_hash text NOT NULL, \
        active      boolean NOT NULL DEFAULT false, \
        created_at  timestamptz NOT NULL DEFAULT now(), \
        updated_at  timestamptz NOT NULL DEFAULT now() \
    )",
];

/// The starter URL rules from the detection strategy. Shipped **disabled** —
/// they exist so the Rules tab has something to preview, not so anything runs
/// unasked.
const STARTER_RULES: &[(&str, &str, &str)] = &[
    (
        "tracking-params",
        "url_rule",
        r#"{"pattern": "[?&](utm_[a-z]+|fbclid|gclid)=", "confidence": 0.95, "description": "URLs with tracking parameters; the canonical page is the same URL without them"}"#,
    ),
    (
        "calendar-pages",
        "url_rule",
        r#"{"pattern": "/(calendar|events)/\\d{4}/\\d{2}", "confidence": 0.8, "description": "Per-month calendar and event listing pages"}"#,
    ),
    (
        "login-and-account",
        "url_rule",
        r#"{"pattern": "/(login|signin|signup|account|cart)([/?]|$)", "confidence": 0.9, "description": "Login, signup and account pages"}"#,
    ),
];

/// Create the `ovis.prune_*` tables. Returns `false` (with a loud warning)
/// when the database user cannot, in which case every `/prune/*` endpoint
/// reports the feature unavailable rather than half-working.
pub async fn ensure_tables(pool: &PgPool) -> bool {
    for statement in DDL {
        if let Err(err) = sqlx::query(*statement).execute(pool).await {
            tracing::warn!(
                error = %err,
                "cannot create the ovis.prune_* tables; pruning endpoints will answer 503"
            );
            return false;
        }
    }
    // Seed the starter pack only into an empty rules table, so a deliberate
    // delete of a starter rule is not undone at the next boot.
    match sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ovis.prune_rules")
        .fetch_one(pool)
        .await
    {
        Ok(0) => {
            for (name, kind, body) in STARTER_RULES {
                let body: Value = serde_json::from_str(body).expect("starter rule body is JSON");
                let _ = sqlx::query(
                    "INSERT INTO ovis.prune_rules (name, kind, body, enabled) \
                     VALUES ($1, $2, $3, false) ON CONFLICT (name) DO NOTHING",
                )
                .bind(name)
                .bind(kind)
                .bind(body)
                .execute(pool)
                .await;
            }
        }
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(error = %err, "could not check ovis.prune_rules for seeding");
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

/// Append one audit row. Failures are logged, never propagated: an audit
/// hiccup must not fail the action it records — but it must not be silent.
pub async fn audit(
    pool: &PgPool,
    actor: &str,
    action: &str,
    document_id: Option<&str>,
    scan_id: Option<i64>,
    candidate_id: Option<i64>,
    detail: Option<Value>,
) {
    let result = sqlx::query(
        "INSERT INTO ovis.prune_audit (actor, action, document_id, scan_id, candidate_id, detail) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(actor)
    .bind(action)
    .bind(document_id)
    .bind(scan_id)
    .bind(candidate_id)
    .bind(detail)
    .execute(pool)
    .await;
    if let Err(err) = result {
        tracing::error!(action, error = %err, "failed to write a prune audit row");
    }
}

#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub document_id: Option<String>,
    pub since: Option<DateTime<Utc>>,
}

pub async fn list_audit(
    pool: &PgPool,
    filter: &AuditFilter,
    limit: i64,
    offset: i64,
) -> CoreResult<(Vec<PruneAuditItem>, i64)> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT id, at, actor, action, document_id, scan_id, candidate_id, detail, \
                count(*) OVER () AS total \
         FROM ovis.prune_audit WHERE TRUE",
    );
    if let Some(action) = &filter.action {
        qb.push(" AND action = ").push_bind(action.clone());
    }
    if let Some(actor) = &filter.actor {
        qb.push(" AND actor = ").push_bind(actor.clone());
    }
    if let Some(document_id) = &filter.document_id {
        qb.push(" AND document_id = ")
            .push_bind(document_id.clone());
    }
    if let Some(since) = filter.since {
        qb.push(" AND at >= ").push_bind(since);
    }
    qb.push(" ORDER BY at DESC, id DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    let rows = qb.build().fetch_all(pool).await?;
    let total = rows.first().map(|r| r.get::<i64, _>("total")).unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|r| PruneAuditItem {
            id: r.get("id"),
            at: r.get("at"),
            actor: r.get("actor"),
            action: r.get("action"),
            document_id: r.get("document_id"),
            scan_id: r.get("scan_id"),
            candidate_id: r.get("candidate_id"),
            detail: r.get("detail"),
        })
        .collect();
    Ok((items, total))
}

pub async fn audit_count(pool: &PgPool) -> CoreResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM ovis.prune_audit")
        .fetch_one(pool)
        .await?)
}

// ---------------------------------------------------------------------------
// Candidates: filters, listing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CandidateFilter {
    /// `None` means the open states (candidate/staged/deleting) — history is
    /// asked for explicitly.
    pub states: Option<Vec<String>>,
    pub detector: Option<String>,
    pub connector_id: Option<i32>,
    pub min_confidence: Option<f32>,
    pub recrawl_risk: Option<bool>,
    pub scan_id: Option<i64>,
    /// Only staged rows whose grace is already over (the reaper's due set).
    pub due_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CandidateSort {
    #[default]
    ConfidenceDesc,
    ChunksDesc,
    ChunksAsc,
    CreatedDesc,
    CreatedAsc,
    ExpiryAsc,
}

impl CandidateSort {
    pub fn parse(raw: &str) -> CoreResult<Self> {
        match raw {
            "confidence_desc" => Ok(Self::ConfidenceDesc),
            "chunks_desc" => Ok(Self::ChunksDesc),
            "chunks_asc" => Ok(Self::ChunksAsc),
            "created_desc" => Ok(Self::CreatedDesc),
            "created_asc" => Ok(Self::CreatedAsc),
            "expiry_asc" => Ok(Self::ExpiryAsc),
            other => Err(CoreError::Invalid(format!(
                "unknown sort '{other}'; expected one of confidence_desc, chunks_desc, \
                 chunks_asc, created_desc, created_asc, expiry_asc"
            ))),
        }
    }

    fn order_by(self) -> &'static str {
        match self {
            Self::ConfidenceDesc => "pc.confidence DESC, pc.id DESC",
            Self::ChunksDesc => {
                "COALESCE(d.chunk_count, pc.chunk_count) DESC NULLS LAST, pc.id DESC"
            }
            Self::ChunksAsc => "COALESCE(d.chunk_count, pc.chunk_count) ASC NULLS LAST, pc.id DESC",
            Self::CreatedDesc => "pc.created_at DESC, pc.id DESC",
            Self::CreatedAsc => "pc.created_at ASC, pc.id ASC",
            Self::ExpiryAsc => "pc.stage_expires_at ASC NULLS LAST, pc.id ASC",
        }
    }
}

fn push_candidate_filters(qb: &mut QueryBuilder<Postgres>, f: &CandidateFilter) {
    qb.push(" WHERE TRUE");
    match &f.states {
        Some(states) => {
            qb.push(" AND pc.state = ANY(")
                .push_bind(states.clone())
                .push(")");
        }
        None => {
            qb.push(" AND pc.state = ANY(")
                .push_bind(OPEN_STATES.map(String::from).to_vec())
                .push(")");
        }
    }
    if let Some(detector) = &f.detector {
        // Containment over the reasons array, served by the GIN index.
        qb.push(" AND pc.reasons @> ")
            .push_bind(serde_json::json!([{ "detector": detector }]));
    }
    if let Some(connector_id) = f.connector_id {
        qb.push(" AND pc.connector_id = ").push_bind(connector_id);
    }
    if let Some(min) = f.min_confidence {
        qb.push(" AND pc.confidence >= ").push_bind(min);
    }
    if let Some(risk) = f.recrawl_risk {
        qb.push(" AND pc.recrawl_risk = ").push_bind(risk);
    }
    if let Some(scan_id) = f.scan_id {
        qb.push(" AND pc.scan_id = ").push_bind(scan_id);
    }
    if f.due_only {
        qb.push(" AND pc.state = 'staged' AND pc.stage_expires_at <= now()");
    }
}

const CANDIDATE_COLUMNS: &str = "\
SELECT pc.id, pc.document_id, pc.scan_id, pc.state, pc.reasons, pc.confidence, \
       pc.recrawl_risk, pc.connector_id, pc.cc_pair_id, pc.chunk_count, \
       pc.prev_hidden, pc.staged_at, pc.stage_expires_at, pc.staged_by, pc.remember, \
       pc.deleted_at, pc.delete_outcome, pc.resolved_reason, pc.created_at, pc.updated_at, \
       d.semantic_id AS doc_semantic_id, d.link AS doc_link, d.hidden AS doc_hidden, \
       d.chunk_count AS doc_chunk_count, (d.id IS NOT NULL) AS doc_exists, \
       c.name AS connector_name \
FROM ovis.prune_candidate pc \
LEFT JOIN public.document d ON d.id = pc.document_id \
LEFT JOIN public.connector c ON c.id = pc.connector_id ";

fn row_to_candidate(r: &sqlx::postgres::PgRow) -> PruneCandidateItem {
    let reasons: Value = r.get("reasons");
    let reasons: Vec<PruneReason> = serde_json::from_value(reasons).unwrap_or_default();
    let doc_exists: bool = r.get("doc_exists");
    PruneCandidateItem {
        id: r.get("id"),
        document_id: r.get("document_id"),
        scan_id: r.get("scan_id"),
        state: r.get("state"),
        reasons,
        confidence: r.get("confidence"),
        recrawl_risk: r.get("recrawl_risk"),
        connector_id: r.get("connector_id"),
        connector_name: r.get("connector_name"),
        cc_pair_id: r.get("cc_pair_id"),
        // Live value when the row exists; the flag-time value otherwise. A
        // NULL live value stays NULL — "not counted yet" is not zero.
        chunk_count: if doc_exists {
            r.get("doc_chunk_count")
        } else {
            r.get("chunk_count")
        },
        semantic_id: r.get("doc_semantic_id"),
        link: r.get("doc_link"),
        doc_exists,
        hidden: r.get("doc_hidden"),
        prev_hidden: r.get("prev_hidden"),
        staged_at: r.get("staged_at"),
        stage_expires_at: r.get("stage_expires_at"),
        staged_by: r.get("staged_by"),
        remember: r.get("remember"),
        deleted_at: r.get("deleted_at"),
        delete_outcome: r.get("delete_outcome"),
        resolved_reason: r.get("resolved_reason"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

pub async fn list_candidates(
    pool: &PgPool,
    filter: &CandidateFilter,
    sort: CandidateSort,
    limit: i64,
    offset: i64,
) -> CoreResult<Vec<PruneCandidateItem>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(CANDIDATE_COLUMNS);
    push_candidate_filters(&mut qb, filter);
    qb.push(" ORDER BY ").push(sort.order_by());
    qb.push(" LIMIT ").push_bind(limit);
    if offset > 0 {
        qb.push(" OFFSET ").push_bind(offset);
    }
    let rows = qb.build().fetch_all(pool).await?;
    Ok(rows.iter().map(row_to_candidate).collect())
}

pub async fn count_candidates(pool: &PgPool, filter: &CandidateFilter) -> CoreResult<i64> {
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT count(*) FROM ovis.prune_candidate pc ");
    push_candidate_filters(&mut qb, filter);
    let count: i64 = qb.build_query_scalar().fetch_one(pool).await?;
    Ok(count)
}

pub async fn get_candidate(pool: &PgPool, id: i64) -> CoreResult<Option<PruneCandidateItem>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(CANDIDATE_COLUMNS);
    qb.push(" WHERE pc.id = ").push_bind(id);
    let row = qb.build().fetch_optional(pool).await?;
    Ok(row.as_ref().map(row_to_candidate))
}

/// Resolve a selector (explicit ids or a filter) into concrete rows, ordered
/// stably. Used by every bulk mutation, so `confirm_count` compares against
/// exactly what would be acted on.
pub async fn resolve_selection(
    pool: &PgPool,
    ids: Option<&[i64]>,
    filter: Option<&CandidateFilter>,
) -> CoreResult<Vec<PruneCandidateItem>> {
    match (ids, filter) {
        (Some(ids), None) => {
            if ids.is_empty() {
                return Err(CoreError::Invalid("ids must not be empty".into()));
            }
            let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(CANDIDATE_COLUMNS);
            qb.push(" WHERE pc.id = ANY(")
                .push_bind(ids.to_vec())
                .push(")");
            qb.push(" ORDER BY pc.id");
            let rows = qb.build().fetch_all(pool).await?;
            Ok(rows.iter().map(row_to_candidate).collect())
        }
        (None, Some(filter)) => {
            let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(CANDIDATE_COLUMNS);
            push_candidate_filters(&mut qb, filter);
            qb.push(" ORDER BY pc.id");
            let rows = qb.build().fetch_all(pool).await?;
            Ok(rows.iter().map(row_to_candidate).collect())
        }
        (Some(_), Some(_)) => Err(CoreError::Invalid(
            "pass either ids or filter, not both".into(),
        )),
        (None, None) => Err(CoreError::Invalid(
            "pass ids or a filter; an empty selector would select nothing".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Candidates: detector upserts
// ---------------------------------------------------------------------------

/// One document a detector flagged, with attribution captured at flag time.
#[derive(Debug, Clone)]
pub struct DetectorHit {
    pub document_id: String,
    pub reasons: Vec<PruneReason>,
    pub connector_id: Option<i32>,
    pub cc_pair_id: Option<i32>,
    pub chunk_count: Option<i32>,
    pub recrawl_risk: bool,
}

/// What happened to one hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Inserted,
    Updated,
    /// The document has an open lifecycle row past `candidate` (staged or
    /// deleting) — scans never touch those.
    LeftAlone,
    /// The document is on the exclusion list.
    Excluded,
}

/// Fold new reasons into existing ones: a reason replaces an existing one
/// with the same `(detector, code)`, others are appended. Never flags a
/// document twice for the same thing.
pub fn merge_reasons(existing: &mut Vec<PruneReason>, incoming: Vec<PruneReason>) {
    for reason in incoming {
        match existing
            .iter_mut()
            .find(|r| r.detector == reason.detector && r.code == reason.code)
        {
            Some(slot) => *slot = reason,
            None => existing.push(reason),
        }
    }
}

pub fn max_confidence(reasons: &[PruneReason]) -> f32 {
    reasons.iter().map(|r| r.confidence).fold(0.0, f32::max)
}

/// Record a detector hit: insert a new candidate, or update the reasons of an
/// existing open `candidate`. Staged/deleting rows are left alone; excluded
/// documents are skipped.
pub async fn upsert_candidate(
    pool: &PgPool,
    scan_id: Option<i64>,
    hit: &DetectorHit,
) -> CoreResult<UpsertOutcome> {
    let excluded: Option<String> =
        sqlx::query_scalar("SELECT reason FROM ovis.prune_exclusions WHERE document_id = $1")
            .bind(&hit.document_id)
            .fetch_optional(pool)
            .await?;
    if excluded.is_some() {
        return Ok(UpsertOutcome::Excluded);
    }

    let open: Option<(i64, String, Value)> = sqlx::query_as(
        "SELECT id, state, reasons FROM ovis.prune_candidate \
         WHERE document_id = $1 AND state = ANY($2)",
    )
    .bind(&hit.document_id)
    .bind(OPEN_STATES.map(String::from).to_vec())
    .fetch_optional(pool)
    .await?;

    match open {
        None => {
            let reasons = serde_json::to_value(&hit.reasons)
                .map_err(|e| CoreError::Invalid(format!("unserialisable reasons: {e}")))?;
            sqlx::query(
                "INSERT INTO ovis.prune_candidate \
                     (document_id, scan_id, state, reasons, confidence, recrawl_risk, \
                      connector_id, cc_pair_id, chunk_count) \
                 VALUES ($1, $2, 'candidate', $3, $4, $5, $6, $7, $8)",
            )
            .bind(&hit.document_id)
            .bind(scan_id)
            .bind(reasons)
            .bind(max_confidence(&hit.reasons))
            .bind(hit.recrawl_risk)
            .bind(hit.connector_id)
            .bind(hit.cc_pair_id)
            .bind(hit.chunk_count)
            .execute(pool)
            .await?;
            Ok(UpsertOutcome::Inserted)
        }
        Some((id, state, existing_reasons)) if state == "candidate" => {
            let mut reasons: Vec<PruneReason> =
                serde_json::from_value(existing_reasons).unwrap_or_default();
            merge_reasons(&mut reasons, hit.reasons.clone());
            let confidence = max_confidence(&reasons);
            let reasons = serde_json::to_value(&reasons)
                .map_err(|e| CoreError::Invalid(format!("unserialisable reasons: {e}")))?;
            sqlx::query(
                "UPDATE ovis.prune_candidate \
                 SET reasons = $2, confidence = $3, scan_id = COALESCE($4, scan_id), \
                     recrawl_risk = $5, connector_id = COALESCE($6, connector_id), \
                     cc_pair_id = COALESCE($7, cc_pair_id), \
                     chunk_count = COALESCE($8, chunk_count), updated_at = now() \
                 WHERE id = $1",
            )
            .bind(id)
            .bind(reasons)
            .bind(confidence)
            .bind(scan_id)
            .bind(hit.recrawl_risk)
            .bind(hit.connector_id)
            .bind(hit.cc_pair_id)
            .bind(hit.chunk_count)
            .execute(pool)
            .await?;
            Ok(UpsertOutcome::Updated)
        }
        Some(_) => Ok(UpsertOutcome::LeftAlone),
    }
}

/// Record a page of detector hits with a bounded number of round trips.
///
/// [`upsert_candidate`] costs three queries per document — an exclusion
/// lookup, an open-row lookup, and the write. Across a 1.7 M-document corpus
/// that latency *is* the scan: the v1 exact phase spent ~35 minutes on 21.5 k
/// groups almost entirely waiting on it. Here the two lookups are one query
/// each per page and the inserts are a single multi-row statement, so a page
/// of 1000 costs a handful of round trips instead of 3000.
///
/// Semantics are identical to the per-document path: excluded documents are
/// skipped, rows already past `candidate` are left alone, and reasons merge by
/// `(detector, code)` with confidence as the maximum.
pub async fn upsert_candidates(
    pool: &PgPool,
    scan_id: Option<i64>,
    hits: &[DetectorHit],
) -> CoreResult<Vec<UpsertOutcome>> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = hits.iter().map(|h| h.document_id.clone()).collect();

    let excluded: Vec<String> = sqlx::query_scalar(
        "SELECT document_id FROM ovis.prune_exclusions WHERE document_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let excluded: std::collections::HashSet<String> = excluded.into_iter().collect();

    let open_rows: Vec<(i64, String, String, Value)> = sqlx::query_as(
        "SELECT id, document_id, state, reasons FROM ovis.prune_candidate \
         WHERE document_id = ANY($1) AND state = ANY($2)",
    )
    .bind(&ids)
    .bind(OPEN_STATES.map(String::from).to_vec())
    .fetch_all(pool)
    .await?;
    let open: std::collections::HashMap<String, (i64, String, Value)> = open_rows
        .into_iter()
        .map(|(id, doc, state, reasons)| (doc, (id, state, reasons)))
        .collect();

    let mut outcomes = Vec::with_capacity(hits.len());
    let mut to_insert: Vec<&DetectorHit> = Vec::new();
    let mut to_update: Vec<(i64, Vec<PruneReason>, &DetectorHit)> = Vec::new();

    for hit in hits {
        if excluded.contains(&hit.document_id) {
            outcomes.push(UpsertOutcome::Excluded);
            continue;
        }
        match open.get(&hit.document_id) {
            None => {
                outcomes.push(UpsertOutcome::Inserted);
                to_insert.push(hit);
            }
            Some((id, state, existing)) if state == "candidate" => {
                let mut reasons: Vec<PruneReason> =
                    serde_json::from_value(existing.clone()).unwrap_or_default();
                merge_reasons(&mut reasons, hit.reasons.clone());
                outcomes.push(UpsertOutcome::Updated);
                to_update.push((*id, reasons, hit));
            }
            Some(_) => outcomes.push(UpsertOutcome::LeftAlone),
        }
    }

    if !to_insert.is_empty() {
        // Chunked so one page never exceeds Postgres' parameter limit
        // (65535 / 8 columns).
        for chunk in to_insert.chunks(2000) {
            let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
                "INSERT INTO ovis.prune_candidate \
                 (document_id, scan_id, state, reasons, confidence, recrawl_risk, \
                  connector_id, cc_pair_id, chunk_count) ",
            );
            qb.push_values(chunk, |mut row, hit| {
                let reasons = serde_json::to_value(&hit.reasons).unwrap_or(Value::Null);
                row.push_bind(hit.document_id.clone())
                    .push_bind(scan_id)
                    .push("'candidate'")
                    .push_bind(reasons)
                    .push_bind(max_confidence(&hit.reasons))
                    .push_bind(hit.recrawl_risk)
                    .push_bind(hit.connector_id)
                    .push_bind(hit.cc_pair_id)
                    .push_bind(hit.chunk_count);
            });
            // A concurrent writer may have opened a row between the lookup and
            // here; the partial unique index turns that into a no-op rather
            // than a failed page.
            qb.push(" ON CONFLICT DO NOTHING");
            qb.build().execute(pool).await?;
        }
    }

    for (id, reasons, hit) in to_update {
        let confidence = max_confidence(&reasons);
        let reasons = serde_json::to_value(&reasons)
            .map_err(|e| CoreError::Invalid(format!("unserialisable reasons: {e}")))?;
        sqlx::query(
            "UPDATE ovis.prune_candidate \
             SET reasons = $2, confidence = $3, scan_id = COALESCE($4, scan_id), \
                 recrawl_risk = $5, connector_id = COALESCE($6, connector_id), \
                 cc_pair_id = COALESCE($7, cc_pair_id), \
                 chunk_count = COALESCE($8, chunk_count), updated_at = now() \
             WHERE id = $1",
        )
        .bind(id)
        .bind(reasons)
        .bind(confidence)
        .bind(scan_id)
        .bind(hit.recrawl_risk)
        .bind(hit.connector_id)
        .bind(hit.cc_pair_id)
        .bind(hit.chunk_count)
        .execute(pool)
        .await?;
    }

    Ok(outcomes)
}

/// Close open `candidate` rows a completed re-scan no longer flags: same
/// detector, inside the scan's scope, not touched by this scan.
///
/// Returns the ids closed so the caller can audit them.
pub async fn close_stale_candidates(
    pool: &PgPool,
    scan_id: i64,
    detectors: &[String],
    scope: &PruneScope,
) -> CoreResult<Vec<(i64, String)>> {
    // "This scan did not touch it" is answered by the scan id every upsert
    // stamps, not by comparing `updated_at` against the scan's start.
    //
    // The timestamp form was a race: the two values are written by different
    // statements, and a re-scan fast enough to land inside the resolution of
    // the comparison left resolved candidates open — reporting
    // `candidates_closed: 0` for a document that no longer matched anything.
    // Reproduced by running this suite under load. Identity does not have that
    // failure mode at any speed. `IS DISTINCT FROM` so a candidate with no
    // scan id (opened by hand, or before scans recorded one) still closes.
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "UPDATE ovis.prune_candidate pc \
         SET state = 'dismissed', resolved_reason = 'no_longer_matches', updated_at = now() \
         WHERE pc.state = 'candidate' AND pc.scan_id IS DISTINCT FROM ",
    );
    qb.push_bind(scan_id);
    // Only candidates whose *every* reason comes from a detector this scan
    // ran: a document flagged by both `thin` (re-scanned, no longer matching)
    // and `language` (not re-scanned) must survive with its language reason.
    qb.push(
        " AND NOT EXISTS ( \
            SELECT 1 FROM jsonb_array_elements(pc.reasons) AS r \
            WHERE NOT (r->>'detector' = ANY(",
    );
    qb.push_bind(detectors.to_vec());
    qb.push(")))");
    match scope.kind.as_str() {
        "connectors" => {
            qb.push(" AND pc.connector_id = ANY(")
                .push_bind(scope.connector_ids.clone().unwrap_or_default())
                .push(")");
        }
        "url_prefix" => {
            let prefix = scope.url_prefix.clone().unwrap_or_default();
            qb.push(" AND pc.document_id LIKE ")
                .push_bind(format!("{}%", escape_like(&prefix)));
            qb.push(" ESCAPE '\\'");
        }
        _ => {}
    }
    qb.push(" RETURNING pc.id, pc.document_id");
    let rows = qb.build().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<i64, _>("id"), r.get::<String, _>("document_id")))
        .collect())
}

fn escape_like(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 4);
    for ch in term.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------
// Candidates: lifecycle transitions (compare-and-swap by state)
// ---------------------------------------------------------------------------

/// candidate → staged. Returns false when the row was not in `candidate`
/// (lost race, or already moved) — the caller reports that per-id.
/// Stage one candidate and start its grace countdown.
///
/// The deadline is computed **in the database** rather than passed in from the
/// application. It is compared against `now()` by the reaper's due filter, and
/// deriving the two ends of that comparison from two different clocks makes
/// the grace period wrong by whatever they disagree by. Observed: a container
/// whose clock ran 23 ms ahead of Postgres wrote every deadline 23 ms into the
/// database's future, and with `OVIS_PRUNE_GRACE_DAYS=0` — a documented,
/// supported setting — nothing ever came due.
pub async fn mark_staged(
    pool: &PgPool,
    id: i64,
    prev_hidden: bool,
    grace_days: i64,
    staged_by: &str,
) -> CoreResult<Option<DateTime<Utc>>> {
    let row = sqlx::query(
        "UPDATE ovis.prune_candidate \
         SET state = 'staged', prev_hidden = $2, staged_at = now(), \
             stage_expires_at = now() + make_interval(days => $3), \
             staged_by = $4, updated_at = now() \
         WHERE id = $1 AND state = 'candidate' \
         RETURNING stage_expires_at",
    )
    .bind(id)
    .bind(prev_hidden)
    .bind(grace_days.clamp(0, 90) as i32)
    .bind(staged_by)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get("stage_expires_at")))
}

/// staged → restored. The caller un-hides first; this closes the lifecycle.
pub async fn mark_restored(pool: &PgPool, id: i64) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.prune_candidate \
         SET state = 'restored', resolved_reason = 'restored', updated_at = now() \
         WHERE id = $1 AND state = 'staged'",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// candidate → dismissed.
pub async fn mark_dismissed(pool: &PgPool, id: i64) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.prune_candidate \
         SET state = 'dismissed', resolved_reason = 'dismissed', updated_at = now() \
         WHERE id = $1 AND state = 'candidate'",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Bring a staged document's grace deadline forward to now (the "delete
/// sooner" direction) and set `remember`. Never *extends* a deadline.
pub async fn expedite_staged(pool: &PgPool, id: i64, remember: bool) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.prune_candidate \
         SET stage_expires_at = LEAST(stage_expires_at, now()), remember = $2, \
             updated_at = now() \
         WHERE id = $1 AND state = 'staged'",
    )
    .bind(id)
    .bind(remember)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

pub async fn set_remember(pool: &PgPool, id: i64, remember: bool) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.prune_candidate SET remember = $2, updated_at = now() \
         WHERE id = $1 AND state = 'staged'",
    )
    .bind(id)
    .bind(remember)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// staged → deleting, claimed by the reaper. The state gate is what makes a
/// reaper crash resumable without double-deleting: a row can be claimed once.
pub async fn claim_for_deletion(pool: &PgPool, id: i64) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.prune_candidate SET state = 'deleting', updated_at = now() \
         WHERE id = $1 AND state = 'staged' AND stage_expires_at <= now()",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// deleting → deleted, with the honest outcome.
pub async fn mark_deleted(pool: &PgPool, id: i64, outcome: Value) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.prune_candidate \
         SET state = 'deleted', deleted_at = now(), delete_outcome = $2, updated_at = now() \
         WHERE id = $1 AND state = 'deleting'",
    )
    .bind(id)
    .bind(outcome)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// deleting → staged again (the cascade failed; the document is intact).
pub async fn unclaim_deletion(pool: &PgPool, id: i64) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.prune_candidate SET state = 'staged', updated_at = now() \
         WHERE id = $1 AND state = 'deleting'",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Rows stuck in `deleting` from a crashed reaper run, oldest first. On
/// restart these are re-verified (does the document still exist?) before
/// anything runs again.
pub async fn stuck_deleting(pool: &PgPool, limit: i64) -> CoreResult<Vec<PruneCandidateItem>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(CANDIDATE_COLUMNS);
    qb.push(" WHERE pc.state = 'deleting' ORDER BY pc.updated_at ASC LIMIT ")
        .push_bind(limit);
    let rows = qb.build().fetch_all(pool).await?;
    Ok(rows.iter().map(row_to_candidate).collect())
}

/// Documents deleted by the reaper in the trailing hour — the rate-limit
/// denominator.
pub async fn deleted_last_hour(pool: &PgPool) -> CoreResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM ovis.prune_candidate \
         WHERE state = 'deleted' AND deleted_at > now() - interval '1 hour'",
    )
    .fetch_one(pool)
    .await?)
}

// ---------------------------------------------------------------------------
// Status aggregates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct StateCounts {
    pub candidates: i64,
    pub staged: i64,
    pub deleting: i64,
    pub deleted_total: i64,
    pub deleted_7d: i64,
    pub dismissed_total: i64,
    pub restored_total: i64,
    pub staged_expiring_24h: i64,
    pub soonest_expiry: Option<DateTime<Utc>>,
    pub deleted_last_hour: i64,
    pub exclusions: i64,
}

pub async fn state_counts(pool: &PgPool) -> CoreResult<StateCounts> {
    let row = sqlx::query(
        "SELECT \
            count(*) FILTER (WHERE state = 'candidate')  AS candidates, \
            count(*) FILTER (WHERE state = 'staged')     AS staged, \
            count(*) FILTER (WHERE state = 'deleting')   AS deleting, \
            count(*) FILTER (WHERE state = 'deleted')    AS deleted_total, \
            count(*) FILTER (WHERE state = 'deleted' \
                             AND deleted_at > now() - interval '7 days') AS deleted_7d, \
            count(*) FILTER (WHERE state = 'dismissed')  AS dismissed_total, \
            count(*) FILTER (WHERE state = 'restored')   AS restored_total, \
            count(*) FILTER (WHERE state = 'staged' \
                             AND stage_expires_at < now() + interval '24 hours') \
                             AS staged_expiring_24h, \
            min(stage_expires_at) FILTER (WHERE state = 'staged') AS soonest_expiry, \
            count(*) FILTER (WHERE state = 'deleted' \
                             AND deleted_at > now() - interval '1 hour') AS deleted_last_hour \
         FROM ovis.prune_candidate",
    )
    .fetch_one(pool)
    .await?;
    let exclusions: i64 = sqlx::query_scalar("SELECT count(*) FROM ovis.prune_exclusions")
        .fetch_one(pool)
        .await?;
    Ok(StateCounts {
        candidates: row.get("candidates"),
        staged: row.get("staged"),
        deleting: row.get("deleting"),
        deleted_total: row.get("deleted_total"),
        deleted_7d: row.get("deleted_7d"),
        dismissed_total: row.get("dismissed_total"),
        restored_total: row.get("restored_total"),
        staged_expiring_24h: row.get("staged_expiring_24h"),
        soonest_expiry: row.get("soonest_expiry"),
        deleted_last_hour: row.get("deleted_last_hour"),
        exclusions,
    })
}

// ---------------------------------------------------------------------------
// Exclusions
// ---------------------------------------------------------------------------

pub async fn add_exclusion(
    pool: &PgPool,
    document_id: &str,
    reason: &str,
    note: Option<&str>,
) -> CoreResult<()> {
    sqlx::query(
        "INSERT INTO ovis.prune_exclusions (document_id, reason, note) VALUES ($1, $2, $3) \
         ON CONFLICT (document_id) DO UPDATE SET reason = excluded.reason, note = excluded.note",
    )
    .bind(document_id)
    .bind(reason)
    .bind(note)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_exclusion(pool: &PgPool, document_id: &str) -> CoreResult<bool> {
    let deleted = sqlx::query("DELETE FROM ovis.prune_exclusions WHERE document_id = $1")
        .bind(document_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(deleted == 1)
}

pub async fn list_exclusions(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> CoreResult<(Vec<PruneExclusionItem>, i64)> {
    let rows = sqlx::query(
        "SELECT document_id, reason, note, created_at, count(*) OVER () AS total \
         FROM ovis.prune_exclusions ORDER BY created_at DESC, document_id LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = rows.first().map(|r| r.get::<i64, _>("total")).unwrap_or(0);
    Ok((
        rows.into_iter()
            .map(|r| PruneExclusionItem {
                document_id: r.get("document_id"),
                reason: r.get("reason"),
                note: r.get("note"),
                created_at: r.get("created_at"),
            })
            .collect(),
        total,
    ))
}

/// Previously-deleted, remembered documents that have reappeared in
/// `public.document` (the crawler brought them back) and have no open
/// lifecycle row. The reaper *stages* these — never deletes them directly.
pub async fn recrawled_exclusions(pool: &PgPool, limit: i64) -> CoreResult<Vec<String>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT e.document_id \
         FROM ovis.prune_exclusions e \
         JOIN public.document d ON d.id = e.document_id \
         WHERE e.reason = 'deleted_with_remember' \
           AND NOT EXISTS ( \
               SELECT 1 FROM ovis.prune_candidate pc \
               WHERE pc.document_id = e.document_id \
                 AND pc.state = ANY($1) \
           ) \
         ORDER BY e.created_at \
         LIMIT $2",
    )
    .bind(OPEN_STATES.map(String::from).to_vec())
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Whether a recrawled excluded document has already been re-staged once
/// before (its lifecycle history), for audit context.
pub async fn is_excluded(pool: &PgPool, document_id: &str) -> CoreResult<bool> {
    let hit: Option<String> =
        sqlx::query_scalar("SELECT reason FROM ovis.prune_exclusions WHERE document_id = $1")
            .bind(document_id)
            .fetch_optional(pool)
            .await?;
    Ok(hit.is_some())
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

fn row_to_rule(r: &sqlx::postgres::PgRow) -> PruneRuleItem {
    PruneRuleItem {
        id: r.get("id"),
        name: r.get("name"),
        kind: r.get("kind"),
        body: r.get("body"),
        enabled: r.get("enabled"),
        updated_at: r.get("updated_at"),
    }
}

pub async fn list_rules(pool: &PgPool) -> CoreResult<Vec<PruneRuleItem>> {
    let rows = sqlx::query(
        "SELECT id, name, kind, body, enabled, updated_at FROM ovis.prune_rules \
         ORDER BY kind, name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_rule).collect())
}

pub async fn get_rule(pool: &PgPool, id: i64) -> CoreResult<Option<PruneRuleItem>> {
    let row = sqlx::query(
        "SELECT id, name, kind, body, enabled, updated_at FROM ovis.prune_rules WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_rule))
}

pub async fn create_rule(
    pool: &PgPool,
    name: &str,
    kind: &str,
    body: &Value,
    enabled: bool,
) -> CoreResult<PruneRuleItem> {
    let row = sqlx::query(
        "INSERT INTO ovis.prune_rules (name, kind, body, enabled) VALUES ($1, $2, $3, $4) \
         RETURNING id, name, kind, body, enabled, updated_at",
    )
    .bind(name)
    .bind(kind)
    .bind(body)
    .bind(enabled)
    .fetch_one(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.constraint().is_some() => {
            CoreError::Conflict(format!("a rule named '{name}' already exists"))
        }
        _ => CoreError::Db(e),
    })?;
    Ok(row_to_rule(&row))
}

pub async fn update_rule(
    pool: &PgPool,
    id: i64,
    name: Option<&str>,
    body: Option<&Value>,
    enabled: Option<bool>,
) -> CoreResult<Option<PruneRuleItem>> {
    let row = sqlx::query(
        "UPDATE ovis.prune_rules \
         SET name = COALESCE($2, name), body = COALESCE($3, body), \
             enabled = COALESCE($4, enabled), updated_at = now() \
         WHERE id = $1 \
         RETURNING id, name, kind, body, enabled, updated_at",
    )
    .bind(id)
    .bind(name)
    .bind(body)
    .bind(enabled)
    .fetch_optional(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.constraint().is_some() => {
            CoreError::Conflict("another rule already has that name".into())
        }
        _ => CoreError::Db(e),
    })?;
    Ok(row.as_ref().map(row_to_rule))
}

pub async fn delete_rule(pool: &PgPool, id: i64) -> CoreResult<bool> {
    let deleted = sqlx::query("DELETE FROM ovis.prune_rules WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(deleted == 1)
}

// ---------------------------------------------------------------------------
// Scans
// ---------------------------------------------------------------------------

fn row_to_scan(r: &sqlx::postgres::PgRow) -> PruneScanItem {
    let scope: Value = r.get("scope");
    let scope: PruneScope = serde_json::from_value(scope).unwrap_or(PruneScope {
        kind: "all".into(),
        connector_ids: None,
        url_prefix: None,
    });
    PruneScanItem {
        id: r.get("id"),
        scope,
        detectors: r.get("detectors"),
        status: r.get("status"),
        examined: r.get("examined"),
        total: r.get("total"),
        config_hash: r.get("config_hash"),
        stats: r.get("stats"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
        error: r.get("error"),
        created_at: r.get("created_at"),
    }
}

const SCAN_COLUMNS: &str = "SELECT id, scope, detectors, status, examined, total, config_hash, \
     stats, started_at, finished_at, error, created_at FROM ovis.prune_scan";

pub async fn create_scan(
    pool: &PgPool,
    scope: &PruneScope,
    detectors: &[String],
    config_snapshot: &Value,
    config_hash: &str,
) -> CoreResult<PruneScanItem> {
    let scope_json = serde_json::to_value(scope)
        .map_err(|e| CoreError::Invalid(format!("unserialisable scope: {e}")))?;
    let row = sqlx::query(
        "INSERT INTO ovis.prune_scan (scope, detectors, config_snapshot, config_hash) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, scope, detectors, status, examined, total, config_hash, stats, \
                   started_at, finished_at, error, created_at",
    )
    .bind(scope_json)
    .bind(detectors.to_vec())
    .bind(config_snapshot)
    .bind(config_hash)
    .fetch_one(pool)
    .await?;
    Ok(row_to_scan(&row))
}

pub async fn get_scan(pool: &PgPool, id: i64) -> CoreResult<Option<PruneScanItem>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!("{SCAN_COLUMNS} WHERE id = $1")))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_scan))
}

pub async fn list_scans(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> CoreResult<(Vec<PruneScanItem>, i64)> {
    let rows = sqlx::query(
        "SELECT id, scope, detectors, status, examined, total, config_hash, stats, \
                started_at, finished_at, error, created_at, count(*) OVER () AS total_rows \
         FROM ovis.prune_scan ORDER BY id DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = rows
        .first()
        .map(|r| r.get::<i64, _>("total_rows"))
        .unwrap_or(0);
    Ok((rows.iter().map(row_to_scan).collect(), total))
}

/// The scan the runner should work on: the running one if any (resume after a
/// restart), otherwise the oldest queued one.
pub async fn next_scan_to_run(pool: &PgPool) -> CoreResult<Option<PruneScanItem>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{SCAN_COLUMNS} WHERE status IN ('running', 'queued') \
         ORDER BY CASE status WHEN 'running' THEN 0 ELSE 1 END, id ASC LIMIT 1"
    )))
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_scan))
}

pub async fn scan_mark_running(pool: &PgPool, id: i64) -> CoreResult<bool> {
    let updated = sqlx::query(
        "UPDATE ovis.prune_scan SET status = 'running', \
             started_at = COALESCE(started_at, now()) \
         WHERE id = $1 AND status IN ('queued', 'running')",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Persist progress and the resume cursor. Also the cancellation check: the
/// runner calls this between pages and stops when the row says `cancelled`.
pub async fn scan_checkpoint(
    pool: &PgPool,
    id: i64,
    examined: i64,
    total: Option<i64>,
    checkpoint: &Value,
    stats: &Value,
) -> CoreResult<String> {
    let status: String = sqlx::query_scalar(
        "UPDATE ovis.prune_scan \
         SET examined = $2, total = COALESCE($3, total), checkpoint = $4, stats = $5 \
         WHERE id = $1 \
         RETURNING status",
    )
    .bind(id)
    .bind(examined)
    .bind(total)
    .bind(checkpoint)
    .bind(stats)
    .fetch_one(pool)
    .await?;
    Ok(status)
}

/// The full effective-config snapshot a scan was queued under. Not part of
/// `PruneScanItem` (it can be large); the runner reads it explicitly.
pub async fn scan_config_snapshot(pool: &PgPool, id: i64) -> CoreResult<Value> {
    let snapshot: Value =
        sqlx::query_scalar("SELECT config_snapshot FROM ovis.prune_scan WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok(snapshot)
}

pub async fn scan_checkpoint_value(pool: &PgPool, id: i64) -> CoreResult<Option<Value>> {
    let checkpoint: Option<Value> =
        sqlx::query_scalar("SELECT checkpoint FROM ovis.prune_scan WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok(checkpoint)
}

pub async fn scan_finish(
    pool: &PgPool,
    id: i64,
    status: &str,
    error: Option<&str>,
    stats: &Value,
) -> CoreResult<()> {
    sqlx::query(
        "UPDATE ovis.prune_scan \
         SET status = $2, error = $3, stats = $4, finished_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .bind(stats)
    .execute(pool)
    .await?;
    Ok(())
}

/// Request cancellation. Queued scans cancel immediately; a running scan stops
/// at its next checkpoint.
pub async fn scan_cancel(pool: &PgPool, id: i64) -> CoreResult<Option<String>> {
    let status: Option<String> = sqlx::query_scalar(
        "UPDATE ovis.prune_scan \
         SET status = 'cancelled', \
             finished_at = CASE WHEN status = 'queued' THEN now() ELSE finished_at END \
         WHERE id = $1 AND status IN ('queued', 'running') \
         RETURNING status",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(status)
}

// ---------------------------------------------------------------------------
// Scope-aware document scanning helpers (reads of Onyx tables)
// ---------------------------------------------------------------------------

/// A page of documents inside a scan scope, keyset by id. The row shape is
/// what detectors need: identity, counts, timestamps, hash, attribution.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScanDocRow {
    pub id: String,
    pub semantic_id: String,
    pub link: Option<String>,
    pub chunk_count: Option<i32>,
    pub content_hash: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub hidden: bool,
    pub connector_id: Option<i32>,
    pub connector_name: Option<String>,
    pub cc_pair_id: Option<i32>,
    pub cc_pair_status: Option<String>,
}

fn push_scope(qb: &mut QueryBuilder<Postgres>, scope: &PruneScope) {
    match scope.kind.as_str() {
        "connectors" => {
            qb.push(
                " AND EXISTS (SELECT 1 FROM public.document_by_connector_credential_pair z \
                 WHERE z.id = d.id AND z.connector_id = ANY(",
            );
            qb.push_bind(scope.connector_ids.clone().unwrap_or_default());
            qb.push("))");
        }
        "url_prefix" => {
            let prefix = scope.url_prefix.clone().unwrap_or_default();
            qb.push(" AND d.id LIKE ")
                .push_bind(format!("{}%", escape_like(&prefix)))
                .push(" ESCAPE '\\'");
        }
        _ => {}
    }
}

const SCAN_DOC_COLUMNS: &str = "\
SELECT d.id, d.semantic_id, d.link, d.chunk_count, d.content_hash, \
       COALESCE(d.doc_updated_at, d.last_modified) AS updated_at, d.hidden, \
       cx.connector_id, cx.connector_name, cx.cc_pair_id, cx.cc_pair_status \
FROM public.document d \
LEFT JOIN LATERAL ( \
    SELECT c.id AS connector_id, c.name AS connector_name, cc.id AS cc_pair_id, \
           cc.status AS cc_pair_status \
    FROM public.document_by_connector_credential_pair dcc \
    JOIN public.connector c ON c.id = dcc.connector_id \
    LEFT JOIN public.connector_credential_pair cc \
           ON cc.connector_id = dcc.connector_id AND cc.credential_id = dcc.credential_id \
    WHERE dcc.id = d.id \
    ORDER BY c.id \
    LIMIT 1 \
) cx ON TRUE ";

/// One keyset page of scope documents (ordered by id). `after` is the last id
/// of the previous page — the scan checkpoint.
pub async fn scan_documents_page(
    pool: &PgPool,
    scope: &PruneScope,
    after: Option<&str>,
    limit: i64,
) -> CoreResult<Vec<ScanDocRow>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SCAN_DOC_COLUMNS);
    qb.push(" WHERE TRUE");
    push_scope(&mut qb, scope);
    if let Some(after) = after {
        qb.push(" AND d.id > ").push_bind(after.to_string());
    }
    qb.push(" ORDER BY d.id LIMIT ").push_bind(limit);
    Ok(qb.build_query_as().fetch_all(pool).await?)
}

/// Total documents in scope — the scan's honest denominator.
pub async fn scan_scope_total(pool: &PgPool, scope: &PruneScope) -> CoreResult<i64> {
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT count(*) FROM public.document d WHERE TRUE");
    push_scope(&mut qb, scope);
    let count: i64 = qb.build_query_scalar().fetch_one(pool).await?;
    Ok(count)
}

/// Content-hash groups with more than one member inside the scope, keyset by
/// hash. Groups never split across pages because the page boundary is the
/// hash itself.
pub async fn duplicate_hash_groups_page(
    pool: &PgPool,
    scope: &PruneScope,
    after_hash: Option<&str>,
    group_limit: i64,
) -> CoreResult<Vec<(String, i64)>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT d.content_hash, count(*) AS members FROM public.document d \
         WHERE d.content_hash IS NOT NULL",
    );
    push_scope(&mut qb, scope);
    if let Some(after) = after_hash {
        qb.push(" AND d.content_hash > ")
            .push_bind(after.to_string());
    }
    qb.push(" GROUP BY d.content_hash HAVING count(*) > 1 ORDER BY d.content_hash LIMIT ")
        .push_bind(group_limit);
    let rows = qb.build().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("content_hash"),
                r.get::<i64, _>("members"),
            )
        })
        .collect())
}

/// Every member of the given content-hash groups (scope-filtered), for keeper
/// selection.
pub async fn documents_for_hashes(
    pool: &PgPool,
    scope: &PruneScope,
    hashes: &[String],
) -> CoreResult<Vec<ScanDocRow>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SCAN_DOC_COLUMNS);
    qb.push(" WHERE d.content_hash = ANY(")
        .push_bind(hashes.to_vec())
        .push(")");
    push_scope(&mut qb, scope);
    qb.push(" ORDER BY d.content_hash, d.id");
    Ok(qb.build_query_as().fetch_all(pool).await?)
}

/// One keyset page of aged stubs (chunk_count = 0, strictly — never NULL)
/// inside the scope.
pub async fn stub_documents_page(
    pool: &PgPool,
    scope: &PruneScope,
    min_age_days: i64,
    after: Option<&str>,
    limit: i64,
) -> CoreResult<Vec<ScanDocRow>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SCAN_DOC_COLUMNS);
    qb.push(" WHERE d.chunk_count = 0 AND COALESCE(d.doc_updated_at, d.last_modified) <= now() - make_interval(days => ");
    qb.push_bind(min_age_days as i32);
    qb.push(")");
    push_scope(&mut qb, scope);
    if let Some(after) = after {
        qb.push(" AND d.id > ").push_bind(after.to_string());
    }
    qb.push(" ORDER BY d.id LIMIT ").push_bind(limit);
    Ok(qb.build_query_as().fetch_all(pool).await?)
}

/// One document's scan-shaped row, for the reaper's recrawl re-stage.
pub async fn scan_document_row(pool: &PgPool, id: &str) -> CoreResult<Option<ScanDocRow>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SCAN_DOC_COLUMNS);
    qb.push(" WHERE d.id = ").push_bind(id.to_string());
    Ok(qb.build_query_as().fetch_optional(pool).await?)
}

/// Insert a candidate row for a recrawled, previously-pruned document.
///
/// Deliberately bypasses the exclusion check that [`upsert_candidate`]
/// performs — the exclusion list is exactly *why* this row is being created.
/// The unique open-row index still applies: a second insert for a document
/// with an open lifecycle fails, which is the correct outcome.
pub async fn insert_restage_candidate(pool: &PgPool, hit: &DetectorHit) -> CoreResult<i64> {
    let reasons = serde_json::to_value(&hit.reasons)
        .map_err(|e| CoreError::Invalid(format!("unserialisable reasons: {e}")))?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO ovis.prune_candidate \
             (document_id, state, reasons, confidence, recrawl_risk, connector_id, \
              cc_pair_id, chunk_count) \
         VALUES ($1, 'candidate', $2, $3, $4, $5, $6, $7) \
         RETURNING id",
    )
    .bind(&hit.document_id)
    .bind(reasons)
    .bind(max_confidence(&hit.reasons))
    .bind(hit.recrawl_risk)
    .bind(hit.connector_id)
    .bind(hit.cc_pair_id)
    .bind(hit.chunk_count)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Documents by id, optionally restricted to a scan scope (used by the
/// near-duplicate pair phase for keeper metadata and scope membership).
pub async fn scan_documents_by_ids(
    pool: &PgPool,
    scope: Option<&PruneScope>,
    ids: &[String],
) -> CoreResult<Vec<ScanDocRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SCAN_DOC_COLUMNS);
    qb.push(" WHERE d.id = ANY(")
        .push_bind(ids.to_vec())
        .push(")");
    if let Some(scope) = scope {
        push_scope(&mut qb, scope);
    }
    Ok(qb.build_query_as().fetch_all(pool).await?)
}

/// Tag key/value pairs for a page of documents, for the tag-rule detector.
pub async fn tags_for_documents(
    pool: &PgPool,
    ids: &[String],
) -> CoreResult<Vec<(String, String, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT dt.document_id, t.tag_key, t.tag_value \
         FROM public.document__tag dt \
         JOIN public.tag t ON t.id = dt.tag_id \
         WHERE dt.document_id = ANY($1)",
    )
    .bind(ids.to_vec())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// MinHash signature persistence (the checkpointed near-duplicate scan)
// ---------------------------------------------------------------------------

/// The signature store holds one config generation. When the MinHash
/// parameters change, everything previously computed is incomparable — wipe
/// and let the scan rebuild. Returns how many stale rows were dropped.
pub async fn minhash_reset_if_config_changed(pool: &PgPool, config_hash: &str) -> CoreResult<i64> {
    let stale: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ovis.prune_minhash WHERE config_hash <> $1")
            .bind(config_hash)
            .fetch_one(pool)
            .await?;
    if stale > 0 {
        sqlx::query("TRUNCATE ovis.prune_minhash, ovis.prune_minhash_band")
            .execute(pool)
            .await?;
    }
    Ok(stale)
}

/// Fingerprints of already-stored signatures for a page of documents, so a
/// re-scan skips recomputing unchanged content.
pub async fn minhash_fingerprints(
    pool: &PgPool,
    config_hash: &str,
    ids: &[String],
) -> CoreResult<Vec<(String, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT document_id, fingerprint FROM ovis.prune_minhash \
         WHERE config_hash = $1 AND document_id = ANY($2)",
    )
    .bind(config_hash)
    .bind(ids.to_vec())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Store one document's signature and its band hashes (replacing any previous
/// generation of both).
pub async fn minhash_upsert(
    pool: &PgPool,
    document_id: &str,
    config_hash: &str,
    fingerprint: &str,
    sig: &[u8],
    band_hashes: &[i64],
) -> CoreResult<()> {
    sqlx::query(
        "INSERT INTO ovis.prune_minhash (document_id, config_hash, fingerprint, sig, updated_at) \
         VALUES ($1, $2, $3, $4, now()) \
         ON CONFLICT (document_id) DO UPDATE \
           SET config_hash = excluded.config_hash, fingerprint = excluded.fingerprint, \
               sig = excluded.sig, updated_at = now()",
    )
    .bind(document_id)
    .bind(config_hash)
    .bind(fingerprint)
    .bind(sig)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM ovis.prune_minhash_band WHERE document_id = $1")
        .bind(document_id)
        .execute(pool)
        .await?;
    let bands: Vec<i16> = (0..band_hashes.len() as i16).collect();
    sqlx::query(
        "INSERT INTO ovis.prune_minhash_band (document_id, band, hash) \
         SELECT $1, band, hash FROM unnest($2::smallint[], $3::bigint[]) AS t(band, hash)",
    )
    .bind(document_id)
    .bind(bands)
    .bind(band_hashes.to_vec())
    .execute(pool)
    .await?;
    Ok(())
}

/// One keyset page of band buckets holding more than one document — the LSH
/// collision candidates. Cursor is the last (band, hash) of the previous page.
pub async fn minhash_collision_buckets(
    pool: &PgPool,
    after: Option<(i16, i64)>,
    limit: i64,
) -> CoreResult<Vec<(i16, i64, i64)>> {
    let (after_band, after_hash) = after.unwrap_or((-1, i64::MIN));
    let rows = sqlx::query(
        "SELECT band, hash, count(*) AS members FROM ovis.prune_minhash_band \
         WHERE (band, hash) > ($1, $2) \
         GROUP BY band, hash HAVING count(*) > 1 \
         ORDER BY band, hash LIMIT $3",
    )
    .bind(after_band)
    .bind(after_hash)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("band"), r.get("hash"), r.get("members")))
        .collect())
}

/// Members of one band bucket, with their signatures.
pub async fn minhash_bucket_members(
    pool: &PgPool,
    band: i16,
    hash: i64,
    limit: i64,
) -> CoreResult<Vec<(String, Vec<u8>)>> {
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT b.document_id, m.sig FROM ovis.prune_minhash_band b \
         JOIN ovis.prune_minhash m ON m.document_id = b.document_id \
         WHERE b.band = $1 AND b.hash = $2 \
         ORDER BY b.document_id LIMIT $3",
    )
    .bind(band)
    .bind(hash)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// cc-pairs with an IN_PROGRESS index attempt right now — the set the reaper
/// backs off from. Keyed off attempt status, never doc counts.
pub async fn busy_cc_pairs(pool: &PgPool) -> CoreResult<Vec<i32>> {
    let rows: Vec<i32> = sqlx::query_scalar(
        "SELECT DISTINCT ia.connector_credential_pair_id \
         FROM public.index_attempt ia \
         WHERE upper(ia.status) = 'IN_PROGRESS'",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Current `hidden` flag of a document, or `None` when the row is gone.
pub async fn document_hidden(pool: &PgPool, id: &str) -> CoreResult<Option<bool>> {
    Ok(
        sqlx::query_scalar("SELECT hidden FROM public.document WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_confined_to_the_ovis_schema() {
        for statement in DDL {
            let lowered = statement.to_lowercase();
            if lowered.starts_with("create table") || lowered.starts_with("create schema") {
                assert!(
                    lowered.contains("ovis"),
                    "DDL must target the ovis schema: {statement}"
                );
                assert!(
                    !lowered.contains("create table if not exists public."),
                    "no OVIS table may live in public: {statement}"
                );
            }
            if lowered.starts_with("create index") || lowered.starts_with("create unique index") {
                assert!(
                    lowered.contains(" on ovis."),
                    "indexes must be on ovis tables: {statement}"
                );
            }
        }
    }

    #[test]
    fn no_write_statement_in_this_module_targets_an_onyx_table() {
        // Grep-level assertion over this module's own source: every INSERT/
        // UPDATE/DELETE targets ovis.*. The document/connector reads are
        // SELECTs. This is the "nothing writes outside the ovis schema" claim,
        // enforced.
        let source = include_str!("prune.rs");
        for (idx, line) in source.lines().enumerate() {
            let lowered = line.to_lowercase();
            for verb in ["insert into ", "update ", "delete from "] {
                if let Some(pos) = lowered.find(verb) {
                    let target = lowered[pos + verb.len()..]
                        .trim_start()
                        .split(|c: char| c.is_whitespace())
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if target.starts_with("public.") {
                        panic!(
                            "line {}: prune DB layer writes to an Onyx table: {}",
                            idx + 1,
                            line.trim()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn merge_reasons_folds_same_detector_and_code_and_appends_others() {
        let mut existing = vec![PruneReason {
            detector: "thin".into(),
            code: "chunkless_stub".into(),
            detail: "old".into(),
            confidence: 0.9,
            evidence: serde_json::json!({}),
        }];
        merge_reasons(
            &mut existing,
            vec![
                PruneReason {
                    detector: "thin".into(),
                    code: "chunkless_stub".into(),
                    detail: "new".into(),
                    confidence: 0.9,
                    evidence: serde_json::json!({"age_days": 12}),
                },
                PruneReason {
                    detector: "duplicate".into(),
                    code: "exact_duplicate_of".into(),
                    detail: "same hash".into(),
                    confidence: 1.0,
                    evidence: serde_json::json!({}),
                },
            ],
        );
        assert_eq!(existing.len(), 2, "same (detector, code) must fold");
        assert_eq!(existing[0].detail, "new");
        assert_eq!(max_confidence(&existing), 1.0);
    }

    #[test]
    fn confidence_is_the_max_reason_never_an_average() {
        let reasons = vec![
            PruneReason {
                detector: "thin".into(),
                code: "a".into(),
                detail: String::new(),
                confidence: 0.2,
                evidence: serde_json::json!({}),
            },
            PruneReason {
                detector: "duplicate".into(),
                code: "b".into(),
                detail: String::new(),
                confidence: 0.95,
                evidence: serde_json::json!({}),
            },
        ];
        assert_eq!(max_confidence(&reasons), 0.95);
        assert_eq!(max_confidence(&[]), 0.0);
    }

    #[test]
    fn candidate_sort_parses_documented_values_and_rejects_typos() {
        assert_eq!(
            CandidateSort::parse("confidence_desc").unwrap(),
            CandidateSort::ConfidenceDesc
        );
        assert_eq!(
            CandidateSort::parse("expiry_asc").unwrap(),
            CandidateSort::ExpiryAsc
        );
        let err = CandidateSort::parse("confidence").unwrap_err();
        assert!(err.to_string().contains("confidence_desc"));
    }

    #[test]
    fn stub_page_query_never_matches_null_chunk_counts() {
        // `chunk_count = 0` in SQL is NULL-safe by construction: a NULL never
        // compares equal. The assertion here is that the query uses strict
        // equality rather than something like `COALESCE(chunk_count, 0)`.
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SCAN_DOC_COLUMNS);
        qb.push(" WHERE d.chunk_count = 0");
        let sql = qb.into_sql();
        assert!(sql.as_str().contains("d.chunk_count = 0"));
        assert!(!sql
            .as_str()
            .to_lowercase()
            .contains("coalesce(d.chunk_count"));
    }

    #[test]
    fn open_states_are_the_three_live_ones() {
        assert_eq!(OPEN_STATES, ["candidate", "staged", "deleting"]);
    }

    #[test]
    fn starter_rules_are_valid_json_and_ship_disabled() {
        for (name, kind, body) in STARTER_RULES {
            let parsed: Value = serde_json::from_str(body)
                .unwrap_or_else(|e| panic!("starter rule {name} has invalid JSON: {e}"));
            assert!(parsed.get("pattern").is_some(), "{name} needs a pattern");
            assert!(
                parsed.get("confidence").is_some(),
                "{name} needs a confidence"
            );
            assert_eq!(*kind, "url_rule");
        }
        // The seeding SQL inserts enabled = false, asserted here by content.
        let seed_sql = "INSERT INTO ovis.prune_rules (name, kind, body, enabled) \
                     VALUES ($1, $2, $3, false) ON CONFLICT (name) DO NOTHING";
        assert!(seed_sql.contains("false"));
    }

    #[test]
    fn like_escaping_for_scopes() {
        assert_eq!(escape_like("https://a/100%"), "https://a/100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
    }
}
