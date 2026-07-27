//! Pruning: status, candidate review, rules, and the lifecycle mutations.
//!
//! The safety spine lives here and in `prune_reaper`:
//!
//! * Staging is the only mutation review can perform, and it is the existing
//!   reversible `hidden` primitive — Onyx-synced when a token is configured.
//! * Hard deletion happens **only** in the reaper. Nothing in this module
//!   deletes a document; `schedule_delete` only stages and moves deadlines.
//! * Every bulk mutation resolves its selection first and compares
//!   `confirm_count` against it — a drifted set is a 409 carrying the fresh
//!   count, and nothing is changed.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use ovis_core::api_types::{
    ListResponse, PruneAuditItem, PruneBulkFailure, PruneBulkResponse, PruneCandidateDetail,
    PruneCandidateFilterBody, PruneCandidateItem, PruneDismissRequest, PruneExclusionItem,
    PruneLimits, PrunePairEvidence, PruneReaperStatus, PruneReason, PruneRestoreRequest,
    PruneRuleCreate, PruneRuleItem, PruneRulePatch, PruneRulePreviewMatch,
    PruneRulePreviewResponse, PruneScanItem, PruneScanRequest, PruneScheduleDeleteRequest,
    PruneStageRequest, PruneStatusResponse,
};
use ovis_core::db::documents::{self, DocumentUpdate};
use ovis_core::db::prune as db;
use ovis_prune::PruneConfig;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::state::AppState;

/// Detector names accepted by `POST /prune/scans`.
pub const KNOWN_DETECTORS: [&str; 7] = [
    "exact_duplicate",
    "near_duplicate",
    "language",
    "url_rule",
    "tag_rule",
    "thin",
    "stale",
];

/// The `detector` values that appear on *reasons* (what candidate filtering
/// matches). Both duplicate detectors emit `duplicate`; the reaper's
/// re-stage emits `recrawl`.
pub const REASON_DETECTORS: [&str; 7] = [
    "duplicate",
    "language",
    "url_rule",
    "tag_rule",
    "thin",
    "stale",
    "recrawl",
];

pub const CANDIDATE_STATES: [&str; 6] = [
    "candidate",
    "staged",
    "deleting",
    "deleted",
    "dismissed",
    "restored",
];

/// 501 with a specific message when the ovis schema could not be created.
pub fn guard(state: &AppState) -> Result<(), AppError> {
    if state.prune.enabled {
        Ok(())
    } else {
        Err(AppError::NotAvailable(
            "pruning is unavailable: the ovis.prune_* tables could not be created at startup \
             (the database user lacks CREATE on the ovis schema); see the startup log"
                .into(),
        ))
    }
}

/// Who performed an action, for the audit trail. With a single static bearer
/// token there is no per-user subject; the honest distinction available is
/// authenticated-vs-open.
pub fn actor(state: &AppState) -> &'static str {
    if state.cfg.api_token.is_some() {
        "bearer"
    } else {
        "local"
    }
}

// ---------------------------------------------------------------------------
// Detector configuration
// ---------------------------------------------------------------------------

/// A compiled, enabled user rule.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub name: String,
    pub pattern: String,
    pub regex: regex::Regex,
    pub confidence: f32,
}

/// The full effective detector configuration a scan runs under.
#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub config: PruneConfig,
    pub url_rules: Vec<CompiledRule>,
    pub tag_rules: Vec<CompiledRule>,
}

impl EffectiveConfig {
    /// The snapshot stored on the scan row: everything that shaped detection.
    pub fn snapshot(&self) -> Value {
        json!({
            "config": self.config,
            "url_rules": self.url_rules.iter().map(|r| json!({
                "name": r.name, "pattern": r.pattern, "confidence": r.confidence,
            })).collect::<Vec<_>>(),
            "tag_rules": self.tag_rules.iter().map(|r| json!({
                "name": r.name, "pattern": r.pattern, "confidence": r.confidence,
            })).collect::<Vec<_>>(),
        })
    }

    pub fn hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.snapshot().to_string().as_bytes());
        let digest = hasher.finalize();
        hex_prefix(&digest, 16)
    }
}

fn hex_prefix(bytes: &[u8], take: usize) -> String {
    bytes
        .iter()
        .take(take)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn rule_spec(rule: &PruneRuleItem) -> Result<CompiledRule, AppError> {
    let pattern = rule
        .body
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::BadRequest(format!("rule '{}' has no pattern", rule.name))
        })?;
    let confidence = rule
        .body
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.8) as f32;
    let regex = regex::Regex::new(pattern).map_err(|e| {
        AppError::BadRequest(format!("rule '{}' pattern does not compile: {e}", rule.name))
    })?;
    Ok(CompiledRule {
        name: rule.name.clone(),
        pattern: pattern.to_string(),
        regex,
        confidence,
    })
}

/// Defaults ← enabled `detector_config` rules (in name order) ← per-scan
/// overrides. Enabled url/tag rules ride alongside, compiled.
pub async fn effective_config(
    state: &AppState,
    overrides: Option<&Value>,
) -> Result<EffectiveConfig, AppError> {
    let rules = db::list_rules(&state.db).await?;

    let mut config = PruneConfig::default();
    for rule in rules.iter().filter(|r| r.kind == "detector_config" && r.enabled) {
        config = config
            .with_overrides(&rule.body)
            .map_err(|e| AppError::BadRequest(format!("stored detector config '{}': {e}", rule.name)))?;
    }
    if let Some(overrides) = overrides {
        config = config
            .with_overrides(overrides)
            .map_err(|e| AppError::BadRequest(format!("config_overrides: {e}")))?;
    }

    let mut url_rules = Vec::new();
    let mut tag_rules = Vec::new();
    for rule in rules.iter().filter(|r| r.enabled) {
        match rule.kind.as_str() {
            "url_rule" => url_rules.push(rule_spec(rule)?),
            "tag_rule" => tag_rules.push(rule_spec(rule)?),
            _ => {}
        }
    }

    Ok(EffectiveConfig {
        config,
        url_rules,
        tag_rules,
    })
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

pub async fn status(state: &AppState) -> Result<PruneStatusResponse, AppError> {
    guard(state)?;
    let counts = db::state_counts(&state.db).await?;
    let reaper = state.prune.reaper_state();

    let active_scan = match db::next_scan_to_run(&state.db).await? {
        Some(scan) if scan.status == "running" || scan.status == "queued" => Some(scan),
        _ => None,
    };

    Ok(PruneStatusResponse {
        candidates: counts.candidates,
        staged: counts.staged,
        deleting: counts.deleting,
        deleted_7d: counts.deleted_7d,
        deleted_total: counts.deleted_total,
        dismissed_total: counts.dismissed_total,
        restored_total: counts.restored_total,
        exclusions: counts.exclusions,
        soonest_expiry: counts.soonest_expiry,
        staged_expiring_24h: counts.staged_expiring_24h,
        reaper: PruneReaperStatus {
            enabled: true,
            next_run_at: reaper.next_run_at,
            last_run_at: reaper.last_run_at,
            halted: reaper.halted_reason.is_some(),
            halted_reason: reaper.halted_reason,
            deferred: reaper.deferred,
            deferred_reason: reaper.deferred_reason,
            deleted_last_hour: counts.deleted_last_hour,
        },
        active_scan,
        limits: PruneLimits {
            grace_days: state.cfg.prune_grace_days,
            big_batch: state.cfg.prune_big_batch,
            reaper_batch_size: state.cfg.prune_reaper_batch() as i64,
            max_docs_per_hour: state.cfg.prune_max_docs_per_hour,
            reaper_interval_secs: state.cfg.prune_reaper_interval_secs as i64,
        },
    })
}

// ---------------------------------------------------------------------------
// Candidates: list & detail
// ---------------------------------------------------------------------------

/// Parse the `state` query value: a single state, a comma list, `open`
/// (default) or `all`.
pub fn parse_states(raw: Option<&str>) -> Result<Option<Vec<String>>, AppError> {
    match raw.map(str::trim) {
        None | Some("") | Some("open") => Ok(None),
        Some("all") => Ok(Some(CANDIDATE_STATES.map(String::from).to_vec())),
        Some(csv) => {
            let states: Vec<String> = csv
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            for state in &states {
                if !CANDIDATE_STATES.contains(&state.as_str()) {
                    return Err(AppError::BadRequest(format!(
                        "unknown state '{state}'; expected one of {}, open, all",
                        CANDIDATE_STATES.join(", ")
                    )));
                }
            }
            Ok(Some(states))
        }
    }
}

pub fn validate_confidence(min_confidence: Option<f32>) -> Result<(), AppError> {
    if let Some(c) = min_confidence {
        if !(0.0..=1.0).contains(&c) {
            return Err(AppError::BadRequest(format!(
                "min_confidence must be between 0 and 1, got {c}"
            )));
        }
    }
    Ok(())
}

pub fn filter_from_body(body: &PruneCandidateFilterBody) -> Result<db::CandidateFilter, AppError> {
    validate_confidence(body.min_confidence)?;
    if let Some(detector) = &body.detector {
        if !REASON_DETECTORS.contains(&detector.as_str()) {
            return Err(AppError::BadRequest(format!(
                "unknown detector '{detector}'; candidates are filtered by reason detector — \
                 one of {}",
                REASON_DETECTORS.join(", ")
            )));
        }
    }
    Ok(db::CandidateFilter {
        states: parse_states(body.state.as_deref())?,
        detector: body.detector.clone(),
        connector_id: body.connector_id,
        min_confidence: body.min_confidence,
        recrawl_risk: body.recrawl_risk,
        scan_id: body.scan_id,
        due_only: false,
    })
}

pub async fn list_candidates(
    state: &AppState,
    filter: db::CandidateFilter,
    sort: db::CandidateSort,
    limit: i64,
    page: i64,
) -> Result<ListResponse<PruneCandidateItem>, AppError> {
    guard(state)?;
    let offset = (page.max(1) - 1).saturating_mul(limit);
    super::pages::validate_page_depth(page.max(1), limit)?;

    let items_fut = db::list_candidates(&state.db, &filter, sort, limit, offset);
    let count_fut = db::count_candidates(&state.db, &filter);
    let (items, total) = tokio::try_join!(items_fut, count_fut)?;

    Ok(ListResponse {
        items,
        total,
        total_exact: true,
        page: Some(page.max(1)),
        limit,
        next_cursor: None,
        has_more: offset + limit < total,
    })
}

pub async fn candidate_detail(state: &AppState, id: i64) -> Result<PruneCandidateDetail, AppError> {
    guard(state)?;
    let item = db::get_candidate(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "prune candidate",
            id: id.to_string(),
        })?;

    // Hydrate both sides of a duplicate pair, when one exists.
    let pair = match duplicate_reason(&item.reasons) {
        Some((kept_id, similarity)) => {
            let kept = documents::documents_by_ids(&state.db, &[kept_id.clone()], None)
                .await?
                .into_iter()
                .next();
            Some(PrunePairEvidence {
                kept_id,
                kept,
                similarity,
            })
        }
        None => None,
    };

    let excluded = db::is_excluded(&state.db, &item.document_id).await?;

    Ok(PruneCandidateDetail {
        item,
        pair,
        excluded,
    })
}

fn duplicate_reason(reasons: &[PruneReason]) -> Option<(String, f64)> {
    reasons
        .iter()
        .filter(|r| r.detector == "duplicate")
        .find_map(|r| {
            let kept = r.evidence.get("kept")?.as_str()?.to_string();
            let similarity = r
                .evidence
                .get("similarity")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            Some((kept, similarity))
        })
}

// ---------------------------------------------------------------------------
// Bulk selection + confirm_count
// ---------------------------------------------------------------------------

struct Selection {
    rows: Vec<PruneCandidateItem>,
}

/// Resolve a bulk selector against the states an operation may act on, and
/// enforce the `confirm_count` contract.
///
/// `allowed_states` also becomes the default filter state; an explicit
/// `filter.state` must be a subset of it.
async fn resolve_for(
    state: &AppState,
    op: &'static str,
    ids: Option<&[i64]>,
    filter: Option<&PruneCandidateFilterBody>,
    allowed_states: &[&str],
    confirm_count: Option<i64>,
    confirm_required: bool,
) -> Result<Selection, AppError> {
    let db_filter = match (ids, filter) {
        (None, Some(body)) => {
            let mut f = filter_from_body(body)?;
            match &f.states {
                None => f.states = Some(allowed_states.iter().map(|s| s.to_string()).collect()),
                Some(states) => {
                    for s in states {
                        if !allowed_states.contains(&s.as_str()) {
                            return Err(AppError::BadRequest(format!(
                                "{op} acts on {} rows; filter.state '{s}' is not in that set",
                                allowed_states.join("/")
                            )));
                        }
                    }
                }
            }
            Some(f)
        }
        _ => None,
    };

    let rows = db::resolve_selection(&state.db, ids, db_filter.as_ref()).await?;

    if confirm_required && confirm_count.is_none() {
        return Err(AppError::BadRequest(format!(
            "{op} requires confirm_count (the selection currently matches {} rows)",
            rows.len()
        )));
    }
    if let Some(confirmed) = confirm_count {
        if confirmed != rows.len() as i64 {
            return Err(AppError::Conflict(format!(
                "the selection matches {} rows, not the confirmed {confirmed}; nothing was \
                 changed. Re-check and resend with confirm_count={}",
                rows.len(),
                rows.len()
            )));
        }
    }
    if rows.is_empty() {
        return Err(AppError::BadRequest(format!(
            "{op}: the selection matches no rows"
        )));
    }

    Ok(Selection { rows })
}

fn bulk_failure(row: &PruneCandidateItem, code: &str) -> PruneBulkFailure {
    PruneBulkFailure {
        candidate_id: row.id,
        document_id: row.document_id.clone(),
        code: code.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The hidden primitive (shared by stage / restore / the reaper's re-stage)
// ---------------------------------------------------------------------------

/// Set a document's `hidden` flag through the trusted path: the Onyx API when
/// configured (Onyx syncs its own index), direct SQL + index flag sync
/// otherwise. Returns which path ran.
pub async fn set_hidden(state: &AppState, id: &str, hidden: bool) -> Result<&'static str, AppError> {
    if let Some(onyx) = state.onyx.as_ref() {
        onyx.set_doc_hidden(id, hidden).await?;
        Ok("onyx_api")
    } else {
        let affected = documents::update_document(
            &state.db,
            id,
            &DocumentUpdate {
                hidden: Some(hidden),
                ..Default::default()
            },
        )
        .await?;
        if affected == 0 {
            return Err(AppError::NotFound {
                what: "document",
                id: id.to_string(),
            });
        }
        let runtime = state.runtime();
        if let Err(err) = state
            .os
            .update_document_flags(&runtime.index_name, id, Some(hidden), None)
            .await
        {
            // Postgres holds the truth; the index copy of the flag is advisory
            // and Onyx's own sync will converge it. Say so in the log.
            tracing::warn!(document_id = %id, error = %err, "hidden flag index sync failed");
        }
        Ok("direct_sql")
    }
}

/// When the grace period for a newly staged document ends.
pub fn grace_deadline(state: &AppState, now: DateTime<Utc>) -> DateTime<Utc> {
    now + ChronoDuration::days(state.cfg.prune_grace_days)
}

/// Stage one candidate row: record `prev_hidden`, hide, flip state. Used by
/// the stage endpoint, schedule-delete (for candidates), and the reaper's
/// recrawl re-stage.
pub async fn stage_one(
    state: &AppState,
    row: &PruneCandidateItem,
    expires_at: DateTime<Utc>,
    staged_by: &str,
) -> Result<(&'static str, bool), AppError> {
    let prev_hidden = match db::document_hidden(&state.db, &row.document_id).await? {
        Some(hidden) => hidden,
        None => {
            return Err(AppError::NotFound {
                what: "document",
                id: row.document_id.clone(),
            })
        }
    };

    let via = set_hidden(state, &row.document_id, true).await?;

    let flipped = db::mark_staged(&state.db, row.id, prev_hidden, expires_at, staged_by).await?;
    if !flipped {
        // Lost a race: someone else moved this row while we were hiding. If
        // the row is no longer headed for deletion and the document was
        // visible before, put the flag back the way we found it.
        let current = db::get_candidate(&state.db, row.id).await?;
        let still_lifecycle = current
            .as_ref()
            .map(|c| c.state == "staged" || c.state == "deleting")
            .unwrap_or(false);
        if !still_lifecycle && !prev_hidden {
            let _ = set_hidden(state, &row.document_id, false).await;
        }
        return Err(AppError::Conflict(format!(
            "candidate {} changed state while staging; nothing further was done",
            row.id
        )));
    }
    Ok((via, prev_hidden))
}

// ---------------------------------------------------------------------------
// Lifecycle mutations
// ---------------------------------------------------------------------------

pub async fn stage(
    state: &AppState,
    request: PruneStageRequest,
) -> Result<PruneBulkResponse, AppError> {
    guard(state)?;
    let selection = resolve_for(
        state,
        "stage",
        request.ids.as_deref(),
        request.filter.as_ref(),
        &["candidate"],
        Some(request.confirm_count),
        true,
    )
    .await?;

    let who = actor(state);
    let expires_at = grace_deadline(state, Utc::now());
    let mut changed = 0i64;
    let mut failed = Vec::new();
    let mut via_seen: Option<String> = None;

    for row in &selection.rows {
        if row.state != "candidate" {
            failed.push(bulk_failure(row, "WRONG_STATE"));
            continue;
        }
        match stage_one(state, row, expires_at, who).await {
            Ok((via, prev_hidden)) => {
                changed += 1;
                via_seen = Some(via.to_string());
                db::audit(
                    &state.db,
                    who,
                    "staged",
                    Some(&row.document_id),
                    row.scan_id,
                    Some(row.id),
                    Some(json!({
                        "prev_hidden": prev_hidden,
                        "stage_expires_at": expires_at,
                        "via": via,
                        "recrawl_risk": row.recrawl_risk,
                    })),
                )
                .await;
            }
            Err(err) => failed.push(bulk_failure(row, err.code())),
        }
    }

    state.caches.invalidate_document_scoped().await;

    Ok(PruneBulkResponse {
        success: failed.is_empty(),
        requested: selection.rows.len() as i64,
        changed,
        failed,
        state: "staged".into(),
        boost_hidden_via: via_seen,
        stage_expires_at: Some(expires_at),
    })
}

pub async fn restore(
    state: &AppState,
    request: PruneRestoreRequest,
) -> Result<PruneBulkResponse, AppError> {
    guard(state)?;
    let selection = resolve_for(
        state,
        "restore",
        request.ids.as_deref(),
        request.filter.as_ref(),
        &["staged"],
        request.confirm_count,
        false,
    )
    .await?;

    let who = actor(state);
    let mut changed = 0i64;
    let mut failed = Vec::new();
    let mut via_seen: Option<String> = None;

    for row in &selection.rows {
        if row.state != "staged" {
            failed.push(bulk_failure(row, "WRONG_STATE"));
            continue;
        }
        // Close the lifecycle first so the reaper cannot claim the row while
        // we un-hide; a failed un-hide puts the row back exactly as it was.
        let claimed = db::mark_restored(&state.db, row.id).await?;
        if !claimed {
            failed.push(bulk_failure(row, "CONFLICT"));
            continue;
        }
        let prev_hidden = row.prev_hidden.unwrap_or(false);
        match set_hidden(state, &row.document_id, prev_hidden).await {
            Ok(via) => {
                changed += 1;
                via_seen = Some(via.to_string());
                db::audit(
                    &state.db,
                    who,
                    "restored",
                    Some(&row.document_id),
                    row.scan_id,
                    Some(row.id),
                    Some(json!({ "hidden_restored_to": prev_hidden, "via": via })),
                )
                .await;
            }
            Err(err) => {
                // The document is still hidden; reopen the staged row with its
                // original deadline so the situation is exactly pre-restore.
                let _ = sqlx_reopen_staged(state, row.id).await;
                failed.push(bulk_failure(row, err.code()));
            }
        }
    }

    state.caches.invalidate_document_scoped().await;

    Ok(PruneBulkResponse {
        success: failed.is_empty(),
        requested: selection.rows.len() as i64,
        changed,
        failed,
        state: "restored".into(),
        boost_hidden_via: via_seen,
        stage_expires_at: None,
    })
}

async fn sqlx_reopen_staged(state: &AppState, id: i64) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE ovis.prune_candidate SET state = 'staged', resolved_reason = NULL, \
         updated_at = now() WHERE id = $1 AND state = 'restored'",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

pub async fn dismiss(
    state: &AppState,
    request: PruneDismissRequest,
) -> Result<PruneBulkResponse, AppError> {
    guard(state)?;
    let selection = resolve_for(
        state,
        "dismiss",
        request.ids.as_deref(),
        request.filter.as_ref(),
        &["candidate"],
        request.confirm_count,
        false,
    )
    .await?;

    let who = actor(state);
    let mut changed = 0i64;
    let mut failed = Vec::new();

    for row in &selection.rows {
        if row.state != "candidate" {
            failed.push(bulk_failure(row, "WRONG_STATE"));
            continue;
        }
        if !db::mark_dismissed(&state.db, row.id).await? {
            failed.push(bulk_failure(row, "CONFLICT"));
            continue;
        }
        if request.exclude_future {
            db::add_exclusion(&state.db, &row.document_id, "user_excluded", None).await?;
        }
        changed += 1;
        db::audit(
            &state.db,
            who,
            "dismissed",
            Some(&row.document_id),
            row.scan_id,
            Some(row.id),
            Some(json!({ "exclude_future": request.exclude_future })),
        )
        .await;
    }

    Ok(PruneBulkResponse {
        success: failed.is_empty(),
        requested: selection.rows.len() as i64,
        changed,
        failed,
        state: "dismissed".into(),
        boost_hidden_via: None,
        stage_expires_at: None,
    })
}

/// Schedule deletion. Candidates are staged first — the grace period applies
/// in full; automation never skips the waiting room. Already-staged rows get
/// their deadline brought forward to now ("delete sooner"), still
/// reaper-executed and restorable until the cascade runs.
pub async fn schedule_delete(
    state: &AppState,
    request: PruneScheduleDeleteRequest,
) -> Result<PruneBulkResponse, AppError> {
    guard(state)?;
    let selection = resolve_for(
        state,
        "schedule-delete",
        request.ids.as_deref(),
        request.filter.as_ref(),
        &["candidate", "staged"],
        Some(request.confirm_count),
        true,
    )
    .await?;

    let who = actor(state);
    let expires_at = grace_deadline(state, Utc::now());
    let mut changed = 0i64;
    let mut failed = Vec::new();
    let mut via_seen: Option<String> = None;
    let mut latest_deadline: Option<DateTime<Utc>> = None;

    for row in &selection.rows {
        // Default: remember risky documents; and expediting an already-staged
        // row never *clears* a remember set earlier.
        let remember = request.remember.unwrap_or(row.remember || row.recrawl_risk);
        match row.state.as_str() {
            "candidate" => match stage_one(state, row, expires_at, who).await {
                Ok((via, prev_hidden)) => {
                    let _ = db::set_remember(&state.db, row.id, remember).await;
                    changed += 1;
                    via_seen = Some(via.to_string());
                    latest_deadline = Some(latest_deadline.map_or(expires_at, |d| d.max(expires_at)));
                    db::audit(
                        &state.db,
                        who,
                        "scheduled",
                        Some(&row.document_id),
                        row.scan_id,
                        Some(row.id),
                        Some(json!({
                            "prev_hidden": prev_hidden,
                            "stage_expires_at": expires_at,
                            "remember": remember,
                            "recrawl_risk": row.recrawl_risk,
                            "via": via,
                        })),
                    )
                    .await;
                }
                Err(err) => failed.push(bulk_failure(row, err.code())),
            },
            "staged" => {
                if db::expedite_staged(&state.db, row.id, remember).await? {
                    changed += 1;
                    let now = Utc::now();
                    let effective = row.stage_expires_at.map_or(now, |d| d.min(now));
                    latest_deadline = Some(latest_deadline.map_or(effective, |d| d.max(effective)));
                    db::audit(
                        &state.db,
                        who,
                        "scheduled",
                        Some(&row.document_id),
                        row.scan_id,
                        Some(row.id),
                        Some(json!({
                            "expedited": true,
                            "remember": remember,
                            "recrawl_risk": row.recrawl_risk,
                        })),
                    )
                    .await;
                } else {
                    failed.push(bulk_failure(row, "CONFLICT"));
                }
            }
            _ => failed.push(bulk_failure(row, "WRONG_STATE")),
        }
    }

    state.caches.invalidate_document_scoped().await;

    Ok(PruneBulkResponse {
        success: failed.is_empty(),
        requested: selection.rows.len() as i64,
        changed,
        failed,
        state: "staged".into(),
        boost_hidden_via: via_seen,
        stage_expires_at: latest_deadline,
    })
}

// ---------------------------------------------------------------------------
// Scans
// ---------------------------------------------------------------------------

pub fn validate_scope(scope: &ovis_core::api_types::PruneScope) -> Result<(), AppError> {
    match scope.kind.as_str() {
        "all" => Ok(()),
        "connectors" => {
            if scope
                .connector_ids
                .as_ref()
                .map(|ids| ids.is_empty())
                .unwrap_or(true)
            {
                Err(AppError::BadRequest(
                    "scope kind 'connectors' needs a non-empty connector_ids".into(),
                ))
            } else {
                Ok(())
            }
        }
        "url_prefix" => {
            if scope
                .url_prefix
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .is_none()
            {
                Err(AppError::BadRequest(
                    "scope kind 'url_prefix' needs a non-empty url_prefix".into(),
                ))
            } else {
                Ok(())
            }
        }
        other => Err(AppError::BadRequest(format!(
            "unknown scope kind '{other}'; expected all, connectors or url_prefix"
        ))),
    }
}

pub async fn create_scan(
    state: &AppState,
    request: PruneScanRequest,
) -> Result<PruneScanItem, AppError> {
    guard(state)?;
    validate_scope(&request.scope)?;

    if request.detectors.is_empty() {
        return Err(AppError::BadRequest(
            "detectors must name at least one detector; nothing runs unasked".into(),
        ));
    }
    for detector in &request.detectors {
        if !KNOWN_DETECTORS.contains(&detector.as_str()) {
            return Err(AppError::BadRequest(format!(
                "unknown detector '{detector}'; expected one of {}",
                KNOWN_DETECTORS.join(", ")
            )));
        }
    }

    // One scan at a time: a queued or running scan owns the slot.
    if let Some(active) = db::next_scan_to_run(&state.db).await? {
        if active.status == "running" || active.status == "queued" {
            return Err(AppError::Conflict(format!(
                "scan {} is already {}; cancel it or wait for it to finish",
                active.id, active.status
            )));
        }
    }

    let effective = effective_config(state, request.config_overrides.as_ref()).await?;
    let snapshot = effective.snapshot();
    let hash = effective.hash();

    let scan = db::create_scan(
        &state.db,
        &request.scope,
        &request.detectors,
        &snapshot,
        &hash,
    )
    .await?;

    db::audit(
        &state.db,
        actor(state),
        "scan_queued",
        None,
        Some(scan.id),
        None,
        Some(json!({
            "scope": request.scope,
            "detectors": request.detectors,
            "config_hash": hash,
        })),
    )
    .await;

    state.prune.scan_wake.notify_one();
    Ok(scan)
}

pub async fn list_scans(
    state: &AppState,
    limit: i64,
    page: i64,
) -> Result<ListResponse<PruneScanItem>, AppError> {
    guard(state)?;
    let offset = (page.max(1) - 1).saturating_mul(limit);
    let (items, total) = db::list_scans(&state.db, limit, offset).await?;
    Ok(ListResponse {
        items,
        total,
        total_exact: true,
        page: Some(page.max(1)),
        limit,
        next_cursor: None,
        has_more: offset + limit < total,
    })
}

pub async fn get_scan(state: &AppState, id: i64) -> Result<PruneScanItem, AppError> {
    guard(state)?;
    db::get_scan(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "prune scan",
            id: id.to_string(),
        })
}

pub async fn cancel_scan(state: &AppState, id: i64) -> Result<PruneScanItem, AppError> {
    guard(state)?;
    let existing = get_scan(state, id).await?;
    match db::scan_cancel(&state.db, id).await? {
        Some(_) => {
            db::audit(&state.db, actor(state), "scan_cancelled", None, Some(id), None, None).await;
            get_scan(state, id).await
        }
        None => Err(AppError::Conflict(format!(
            "scan {id} is {}; only queued or running scans can be cancelled",
            existing.status
        ))),
    }
}

// ---------------------------------------------------------------------------
// Audit & exclusions
// ---------------------------------------------------------------------------

pub async fn list_audit(
    state: &AppState,
    filter: db::AuditFilter,
    limit: i64,
    page: i64,
) -> Result<ListResponse<PruneAuditItem>, AppError> {
    guard(state)?;
    let offset = (page.max(1) - 1).saturating_mul(limit);
    let (items, total) = db::list_audit(&state.db, &filter, limit, offset).await?;
    Ok(ListResponse {
        items,
        total,
        total_exact: true,
        page: Some(page.max(1)),
        limit,
        next_cursor: None,
        has_more: offset + limit < total,
    })
}

pub async fn list_exclusions(
    state: &AppState,
    limit: i64,
    page: i64,
) -> Result<ListResponse<PruneExclusionItem>, AppError> {
    guard(state)?;
    let offset = (page.max(1) - 1).saturating_mul(limit);
    let (items, total) = db::list_exclusions(&state.db, limit, offset).await?;
    Ok(ListResponse {
        items,
        total,
        total_exact: true,
        page: Some(page.max(1)),
        limit,
        next_cursor: None,
        has_more: offset + limit < total,
    })
}

pub async fn delete_exclusion(state: &AppState, document_id: &str) -> Result<(), AppError> {
    guard(state)?;
    if !db::remove_exclusion(&state.db, document_id).await? {
        return Err(AppError::NotFound {
            what: "prune exclusion",
            id: document_id.to_string(),
        });
    }
    db::audit(
        &state.db,
        actor(state),
        "exclusion_removed",
        Some(document_id),
        None,
        None,
        None,
    )
    .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

const RULE_KINDS: [&str; 3] = ["url_rule", "tag_rule", "detector_config"];

fn validate_rule_body(kind: &str, body: &Value) -> Result<(), AppError> {
    match kind {
        "url_rule" | "tag_rule" => {
            let pattern = body
                .get("pattern")
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::BadRequest("rule body needs a string 'pattern'".into()))?;
            regex::Regex::new(pattern)
                .map_err(|e| AppError::BadRequest(format!("pattern does not compile: {e}")))?;
            if let Some(confidence) = body.get("confidence") {
                let c = confidence.as_f64().ok_or_else(|| {
                    AppError::BadRequest("confidence must be a number".into())
                })?;
                if !(0.0..=1.0).contains(&c) {
                    return Err(AppError::BadRequest(
                        "confidence must be between 0 and 1".into(),
                    ));
                }
            }
            Ok(())
        }
        "detector_config" => {
            PruneConfig::default()
                .with_overrides(body)
                .map_err(|e| AppError::BadRequest(format!("detector config: {e}")))?;
            Ok(())
        }
        other => Err(AppError::BadRequest(format!(
            "unknown rule kind '{other}'; expected one of {}",
            RULE_KINDS.join(", ")
        ))),
    }
}

pub async fn list_rules(state: &AppState) -> Result<Vec<PruneRuleItem>, AppError> {
    guard(state)?;
    Ok(db::list_rules(&state.db).await?)
}

pub async fn create_rule(
    state: &AppState,
    request: PruneRuleCreate,
) -> Result<PruneRuleItem, AppError> {
    guard(state)?;
    let name = request.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("rule name must not be empty".into()));
    }
    validate_rule_body(&request.kind, &request.body)?;
    let rule = db::create_rule(&state.db, name, &request.kind, &request.body, request.enabled)
        .await?;
    db::audit(
        &state.db,
        actor(state),
        "rule_created",
        None,
        None,
        None,
        Some(json!({ "rule_id": rule.id, "name": rule.name, "kind": rule.kind, "enabled": rule.enabled })),
    )
    .await;
    Ok(rule)
}

pub async fn patch_rule(
    state: &AppState,
    id: i64,
    request: PruneRulePatch,
) -> Result<PruneRuleItem, AppError> {
    guard(state)?;
    let existing = db::get_rule(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "prune rule",
            id: id.to_string(),
        })?;
    if let Some(body) = &request.body {
        validate_rule_body(&existing.kind, body)?;
    }
    if request.name.is_none() && request.body.is_none() && request.enabled.is_none() {
        return Err(AppError::BadRequest(
            "nothing to change: supply name, body or enabled".into(),
        ));
    }
    let rule = db::update_rule(
        &state.db,
        id,
        request.name.as_deref(),
        request.body.as_ref(),
        request.enabled,
    )
    .await?
    .ok_or_else(|| AppError::NotFound {
        what: "prune rule",
        id: id.to_string(),
    })?;
    db::audit(
        &state.db,
        actor(state),
        "rule_updated",
        None,
        None,
        None,
        Some(json!({ "rule_id": rule.id, "name": rule.name, "enabled": rule.enabled })),
    )
    .await;
    Ok(rule)
}

pub async fn delete_rule(state: &AppState, id: i64) -> Result<(), AppError> {
    guard(state)?;
    let existing = db::get_rule(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "prune rule",
            id: id.to_string(),
        })?;
    db::delete_rule(&state.db, id).await?;
    db::audit(
        &state.db,
        actor(state),
        "rule_deleted",
        None,
        None,
        None,
        Some(json!({ "rule_id": id, "name": existing.name })),
    )
    .await;
    Ok(())
}

/// The effective detector config as YAML — what `ovis prune config export`
/// writes to a file.
pub async fn export_config(state: &AppState) -> Result<String, AppError> {
    guard(state)?;
    let effective = effective_config(state, None).await?;
    effective
        .config
        .to_yaml()
        .map_err(|e| AppError::BadRequest(format!("config serialisation failed: {e}")))
}

/// Import a full detector config from YAML: it becomes (or replaces) the
/// enabled `detector_config` rule named `default`. URL/tag rules are separate
/// rows and are not touched by an import.
pub async fn import_config(state: &AppState, yaml: &str) -> Result<PruneRuleItem, AppError> {
    guard(state)?;
    let config = PruneConfig::from_yaml(yaml)
        .map_err(|e| AppError::BadRequest(format!("config YAML does not parse: {e}")))?;
    let body = serde_json::to_value(&config)
        .map_err(|e| AppError::BadRequest(format!("config serialisation failed: {e}")))?;

    let existing = db::list_rules(&state.db)
        .await?
        .into_iter()
        .find(|r| r.kind == "detector_config" && r.name == "default");

    let rule = match existing {
        Some(rule) => db::update_rule(&state.db, rule.id, None, Some(&body), Some(true))
            .await?
            .ok_or_else(|| AppError::NotFound {
                what: "prune rule",
                id: rule.id.to_string(),
            })?,
        None => db::create_rule(&state.db, "default", "detector_config", &body, true).await?,
    };

    db::audit(
        &state.db,
        actor(state),
        "config_imported",
        None,
        None,
        None,
        Some(json!({ "rule_id": rule.id })),
    )
    .await;
    Ok(rule)
}

/// Rows a rule preview will walk before stopping. A preview is a bounded
/// sample, and says so via `complete: false`.
const PREVIEW_SCAN_CAP: i64 = 100_000;
const PREVIEW_PAGE: i64 = 5_000;
const PREVIEW_SAMPLE: usize = 20;

/// Run a rule against live data: sample matches + count, zero mutations.
pub async fn preview_rule(state: &AppState, id: i64) -> Result<PruneRulePreviewResponse, AppError> {
    guard(state)?;
    let rule = db::get_rule(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "prune rule",
            id: id.to_string(),
        })?;

    match rule.kind.as_str() {
        "url_rule" => preview_url_rule(state, &rule).await,
        "tag_rule" => preview_tag_rule(state, &rule).await,
        other => Err(AppError::BadRequest(format!(
            "rules of kind '{other}' have no preview; only url_rule and tag_rule match documents"
        ))),
    }
}

async fn preview_url_rule(
    state: &AppState,
    rule: &PruneRuleItem,
) -> Result<PruneRulePreviewResponse, AppError> {
    let compiled = rule_spec(rule)?;
    let scope = ovis_core::api_types::PruneScope {
        kind: "all".into(),
        connector_ids: None,
        url_prefix: None,
    };

    let mut scanned = 0i64;
    let mut matched = 0i64;
    let mut sample = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let page = db::scan_documents_page(
            &state.db,
            &scope,
            cursor.as_deref(),
            PREVIEW_PAGE.min(PREVIEW_SCAN_CAP - scanned),
        )
        .await?;
        if page.is_empty() {
            break;
        }
        scanned += page.len() as i64;
        cursor = page.last().map(|d| d.id.clone());
        for doc in &page {
            if compiled.regex.is_match(&doc.id) {
                matched += 1;
                if sample.len() < PREVIEW_SAMPLE {
                    sample.push(PruneRulePreviewMatch {
                        document_id: doc.id.clone(),
                        semantic_id: Some(doc.semantic_id.clone()),
                        matched_on: doc.id.clone(),
                    });
                }
            }
        }
        if scanned >= PREVIEW_SCAN_CAP {
            break;
        }
    }

    Ok(PruneRulePreviewResponse {
        matched,
        scanned,
        complete: scanned < PREVIEW_SCAN_CAP,
        sample,
    })
}

async fn preview_tag_rule(
    state: &AppState,
    rule: &PruneRuleItem,
) -> Result<PruneRulePreviewResponse, AppError> {
    let compiled = rule_spec(rule)?;

    // Tag rules match against the (bounded) tag vocabulary, then count the
    // documents carrying any matched tag — far cheaper than walking documents.
    let tags: Vec<(i32, String, String)> = sqlx::query_as(
        "SELECT id, tag_key, tag_value FROM public.tag ORDER BY id LIMIT 100000",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    let scanned = tags.len() as i64;
    let matched_ids: Vec<i32> = tags
        .iter()
        .filter(|(_, key, value)| {
            let kv = format!("{key}={value}");
            compiled.regex.is_match(&kv) || compiled.regex.is_match(value)
        })
        .map(|(id, _, _)| *id)
        .collect();

    if matched_ids.is_empty() {
        return Ok(PruneRulePreviewResponse {
            matched: 0,
            scanned,
            complete: scanned < 100_000,
            sample: Vec::new(),
        });
    }

    let matched: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT dt.document_id) FROM public.document__tag dt \
         WHERE dt.tag_id = ANY($1)",
    )
    .bind(&matched_ids)
    .fetch_one(&state.db)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT dt.document_id, d.semantic_id, t.tag_key, t.tag_value \
         FROM public.document__tag dt \
         JOIN public.tag t ON t.id = dt.tag_id \
         LEFT JOIN public.document d ON d.id = dt.document_id \
         WHERE dt.tag_id = ANY($1) \
         ORDER BY dt.document_id LIMIT $2",
    )
    .bind(&matched_ids)
    .bind(PREVIEW_SAMPLE as i64)
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(PruneRulePreviewResponse {
        matched,
        scanned,
        complete: scanned < 100_000,
        sample: rows
            .into_iter()
            .map(|(document_id, semantic_id, key, value)| PruneRulePreviewMatch {
                document_id,
                semantic_id,
                matched_on: format!("{key}={value}"),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_parse_with_open_default_and_all() {
        assert_eq!(parse_states(None).unwrap(), None);
        assert_eq!(parse_states(Some("open")).unwrap(), None);
        assert_eq!(
            parse_states(Some("all")).unwrap().unwrap().len(),
            CANDIDATE_STATES.len()
        );
        assert_eq!(
            parse_states(Some("staged,deleted")).unwrap().unwrap(),
            vec!["staged".to_string(), "deleted".to_string()]
        );
        assert!(parse_states(Some("stged")).is_err());
    }

    #[test]
    fn rule_bodies_are_validated_per_kind() {
        assert!(validate_rule_body("url_rule", &json!({"pattern": "/tag/", "confidence": 0.8})).is_ok());
        assert!(validate_rule_body("url_rule", &json!({"pattern": "("})).is_err());
        assert!(validate_rule_body("url_rule", &json!({})).is_err());
        assert!(validate_rule_body("url_rule", &json!({"pattern": "x", "confidence": 1.5})).is_err());
        assert!(validate_rule_body("detector_config", &json!({"thin": {"min_age_days": 3}})).is_ok());
        assert!(validate_rule_body("detector_config", &json!({"thn": {}})).is_err());
        assert!(validate_rule_body("nonsense", &json!({})).is_err());
    }

    #[test]
    fn duplicate_reasons_surface_the_pair() {
        let reasons = vec![PruneReason {
            detector: "duplicate".into(),
            code: "exact_duplicate_of".into(),
            detail: "same hash".into(),
            confidence: 1.0,
            evidence: json!({ "kept": "https://a/x", "similarity": 1.0 }),
        }];
        let (kept, sim) = duplicate_reason(&reasons).unwrap();
        assert_eq!(kept, "https://a/x");
        assert_eq!(sim, 1.0);
        assert!(duplicate_reason(&[]).is_none());
    }

    #[test]
    fn config_hash_is_stable_and_sensitive() {
        let a = EffectiveConfig {
            config: PruneConfig::default(),
            url_rules: Vec::new(),
            tag_rules: Vec::new(),
        };
        let b = EffectiveConfig {
            config: PruneConfig::default(),
            url_rules: Vec::new(),
            tag_rules: Vec::new(),
        };
        assert_eq!(a.hash(), b.hash(), "same config, same hash");

        let mut changed = PruneConfig::default();
        changed.dedup.similarity_threshold = 0.95;
        let c = EffectiveConfig {
            config: changed,
            url_rules: Vec::new(),
            tag_rules: Vec::new(),
        };
        assert_ne!(a.hash(), c.hash(), "a threshold change must be visible");
        assert_eq!(a.hash().len(), 32);
    }

    #[test]
    fn scope_validation_names_the_problem() {
        use ovis_core::api_types::PruneScope;
        assert!(validate_scope(&PruneScope {
            kind: "all".into(),
            connector_ids: None,
            url_prefix: None
        })
        .is_ok());
        assert!(validate_scope(&PruneScope {
            kind: "connectors".into(),
            connector_ids: Some(vec![]),
            url_prefix: None
        })
        .is_err());
        assert!(validate_scope(&PruneScope {
            kind: "url_prefix".into(),
            connector_ids: None,
            url_prefix: Some("  ".into())
        })
        .is_err());
        assert!(validate_scope(&PruneScope {
            kind: "everything".into(),
            connector_ids: None,
            url_prefix: None
        })
        .is_err());
    }
}
