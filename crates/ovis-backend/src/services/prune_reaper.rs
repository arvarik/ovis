//! The reaper: the **only** code path that hard-deletes pruned documents.
//!
//! Clients schedule; this task executes — after the grace period, in small
//! batches, rate-limited, backing off while the owning cc-pair indexes, and
//! halting outright when the index is read-only. Every transition lands in
//! `ovis.prune_audit`.
//!
//! Crash safety: a document is claimed by flipping its row `staged →
//! deleting` (a compare-and-swap on state). A reaper that dies mid-batch
//! leaves `deleting` rows behind; the next cycle re-verifies each one against
//! the database — already-gone documents are marked deleted (index cleanup
//! queued), still-present ones go back to `staged` for a clean retry. A row
//! can never be cascade-deleted twice, by state, not by hope.

use std::collections::HashSet;

use chrono::{Duration as ChronoDuration, Utc};
use ovis_core::api_types::{PruneCandidateItem, PruneReason};
use ovis_core::db::prune as db;
use ovis_core::db::trash;
use serde_json::json;

use crate::error::AppError;
use crate::services::prune;
use crate::state::AppState;

const ACTOR: &str = "reaper";

/// `deleting` rows older than this are treated as crash leftovers.
const STUCK_AFTER_MINUTES: i64 = 10;

/// Recrawled exclusions re-staged per cycle. A bound, not a promise — the
/// next cycle picks up the rest.
const RESTAGE_PER_CYCLE: i64 = 200;

pub fn spawn_reaper(state: AppState) {
    let interval = std::time::Duration::from_secs(state.cfg.prune_reaper_interval_secs.max(5));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            state.prune.update_reaper(|s| {
                s.next_run_at = Some(Utc::now() + ChronoDuration::seconds(interval.as_secs() as i64));
            });
            if let Err(err) = run_cycle(&state).await {
                tracing::error!(error = %err.log_detail(), "reaper cycle failed");
            }
            state.prune.update_reaper(|s| s.last_run_at = Some(Utc::now()));
        }
    });
}

/// One reaper cycle. Public so the DB-backed tests can drive cycles directly
/// instead of waiting on timers.
pub async fn run_cycle(state: &AppState) -> Result<CycleReport, AppError> {
    let mut report = CycleReport::default();

    recover_stuck_deleting(state, &mut report).await?;

    // The read-only check gates *everything* that writes documents or the
    // index this cycle — deleting into a read-only index only queues cleanup
    // debt, and staging writes the hidden flag.
    let read_only = check_read_only(state).await;
    let previously_halted = state.prune.reaper_state().halted_reason.is_some();
    match &read_only {
        Some(reason) => {
            if !previously_halted {
                db::audit(
                    &state.db,
                    ACTOR,
                    "halted",
                    None,
                    None,
                    None,
                    Some(json!({ "reason": reason })),
                )
                .await;
                tracing::warn!(reason, "reaper halted");
            }
            let reason = reason.clone();
            state.prune.update_reaper(move |s| s.halted_reason = Some(reason));
            report.halted = true;
            update_metrics(state, &report).await;
            return Ok(report);
        }
        None => {
            if previously_halted {
                db::audit(&state.db, ACTOR, "reaper_resumed", None, None, None, None).await;
                tracing::info!("reaper resumed; the index is writable again");
            }
            state.prune.update_reaper(|s| s.halted_reason = None);
        }
    }

    reap_due(state, &mut report).await?;
    restage_recrawled(state, &mut report).await?;
    if let Err(err) = purge_expired_trash(state, &mut report).await {
        // A purge failure must never abort the cycle: everything above it has
        // already committed, and retained-too-long is the safe direction.
        tracing::warn!(error = %err.log_detail(), "trash purge failed; will retry next cycle");
    }

    let deferred_reason = if report.deferred_indexing > 0 {
        Some("indexing_in_progress".to_string())
    } else if report.deferred_rate > 0 {
        Some("rate_limited".to_string())
    } else {
        None
    };
    let deferred_total = report.deferred_indexing + report.deferred_rate;
    state.prune.update_reaper(move |s| {
        s.deferred = deferred_total;
        s.deferred_reason = deferred_reason;
    });

    if report.deleted > 0 || report.restaged > 0 {
        state.caches.invalidate_document_scoped().await;
    }
    update_metrics(state, &report).await;

    tracing::info!(
        deleted = report.deleted,
        failed = report.failed,
        deferred_indexing = report.deferred_indexing,
        deferred_rate = report.deferred_rate,
        restaged = report.restaged,
        recovered = report.recovered,
        "reaper cycle complete"
    );
    Ok(report)
}

#[derive(Debug, Default, Clone)]
pub struct CycleReport {
    pub deleted: i64,
    pub failed: i64,
    pub deferred_indexing: i64,
    pub deferred_rate: i64,
    pub restaged: i64,
    pub recovered: i64,
    pub purged: i64,
    pub halted: bool,
}

/// `Some(reason)` when deletion must not run.
async fn check_read_only(state: &AppState) -> Option<String> {
    // No trash, no deletion. This is checked before the index because it is
    // the more fundamental refusal: a read-only index is a temporary condition
    // that resolves itself, whereas deleting with nowhere to put the snapshot
    // is unrecoverable by construction.
    if !state.prune.trash_enabled {
        return Some("trash_unavailable".into());
    }
    let index = state.index_name();
    match state.os.index_read_only(&index).await {
        Ok(false) => None,
        Ok(true) => Some("index_read_only".into()),
        // Cannot verify ⇒ do not delete. An unreachable OpenSearch would fail
        // the deletes anyway; halting is the honest shape of that.
        Err(err) => Some(format!("index_status_unknown: {err}")),
    }
}

/// Re-verify rows a crashed run left in `deleting`.
async fn recover_stuck_deleting(
    state: &AppState,
    report: &mut CycleReport,
) -> Result<(), AppError> {
    let stuck = db::stuck_deleting(&state.db, 500).await?;
    let cutoff = Utc::now() - ChronoDuration::minutes(STUCK_AFTER_MINUTES);
    for row in stuck {
        if row.updated_at > cutoff {
            continue; // an active run may legitimately hold this row
        }
        report.recovered += 1;
        let exists = db::document_hidden(&state.db, &row.document_id)
            .await?
            .is_some();
        if exists {
            // The cascade never committed — put it back for a clean retry.
            db::unclaim_deletion(&state.db, row.id).await?;
            db::audit(
                &state.db,
                ACTOR,
                "delete_requeued",
                Some(&row.document_id),
                row.scan_id,
                Some(row.id),
                Some(json!({ "reason": "recovered_after_crash_document_intact" })),
            )
            .await;
        } else {
            // Postgres committed before the crash; the index may still hold
            // chunks. Queue cleanup — the goal state is "no chunks", and the
            // drain task is idempotent about it.
            let _ = ovis_core::db::pending_deletes::enqueue(
                &state.db,
                &row.document_id,
                "reaper crashed between the Postgres commit and index cleanup",
            )
            .await;
            db::mark_deleted(
                &state.db,
                row.id,
                json!({
                    "chunks_deleted": null,
                    "index_cleanup_pending": true,
                    "recovered_after_crash": true,
                }),
            )
            .await?;
            if row.remember {
                remember_exclusion(state, &row).await;
            }
            db::audit(
                &state.db,
                ACTOR,
                "deleted",
                Some(&row.document_id),
                row.scan_id,
                Some(row.id),
                Some(json!({
                    "chunks_deleted": null,
                    "index_cleanup_pending": true,
                    "recovered_after_crash": true,
                })),
            )
            .await;
        }
    }
    Ok(())
}

/// Delete due staged documents, within every limit.
async fn reap_due(state: &AppState, report: &mut CycleReport) -> Result<(), AppError> {
    // Rate window: how much budget is left this hour.
    let deleted_last_hour = db::deleted_last_hour(&state.db).await?;
    let mut budget = (state.cfg.prune_max_docs_per_hour - deleted_last_hour).max(0);

    let due_filter = db::CandidateFilter {
        due_only: true,
        ..Default::default()
    };
    let due_total = db::count_candidates(&state.db, &due_filter).await?;
    if due_total == 0 {
        return Ok(());
    }
    if budget == 0 {
        report.deferred_rate = due_total;
        db::audit(
            &state.db,
            ACTOR,
            "deferred",
            None,
            None,
            None,
            Some(json!({ "count": due_total, "reason": "rate_limited" })),
        )
        .await;
        return Ok(());
    }

    // The schema guard: if Onyx grew a new FK child of document, refuse —
    // exactly like the interactive delete paths do.
    if let Err(err) = state.runtime().delete_is_safe() {
        db::audit(
            &state.db,
            ACTOR,
            "deferred",
            None,
            None,
            None,
            Some(json!({ "count": due_total, "reason": "schema_mismatch", "detail": err.to_string() })),
        )
        .await;
        return Err(err.into());
    }

    let busy: HashSet<i32> = db::busy_cc_pairs(&state.db).await?.into_iter().collect();
    let batch_size = state.cfg.prune_reaper_batch() as i64;
    let index = state.index_name();
    let mut skipped: HashSet<i64> = HashSet::new();
    let mut deleted_this_cycle: i64 = 0;

    'cycle: loop {
        // Refetch due rows each batch; skip the ones this cycle already
        // deferred so the loop always terminates.
        let due = db::list_candidates(
            &state.db,
            &due_filter,
            db::CandidateSort::ExpiryAsc,
            batch_size + skipped.len() as i64,
            0,
        )
        .await?;
        let workable: Vec<_> = due
            .into_iter()
            .filter(|row| !skipped.contains(&row.id))
            .take(batch_size as usize)
            .collect();
        if workable.is_empty() {
            break;
        }

        let mut batch_deleted = 0i64;
        for row in &workable {
            if budget == 0 {
                report.deferred_rate += 1;
                skipped.insert(row.id);
                continue;
            }
            if let Some(cc_pair_id) = row.cc_pair_id {
                if busy.contains(&cc_pair_id) {
                    report.deferred_indexing += 1;
                    skipped.insert(row.id);
                    continue;
                }
            }
            if !db::claim_for_deletion(&state.db, row.id).await? {
                // Restored or already claimed under us; not ours to touch.
                skipped.insert(row.id);
                continue;
            }
            match delete_one(state, &index, row).await {
                Ok(()) => {
                    budget -= 1;
                    deleted_this_cycle += 1;
                    batch_deleted += 1;
                    report.deleted += 1;
                }
                Err(code) => {
                    report.failed += 1;
                    skipped.insert(row.id);
                    let _ = code;
                }
            }
        }

        if batch_deleted == 0 {
            break 'cycle; // everything left is deferred or failing
        }
        // Gentle pressure: breathe between batches.
        tokio::time::sleep(std::time::Duration::from_millis(
            state.cfg.prune_reaper_pause_ms,
        ))
        .await;
    }

    if report.deferred_indexing > 0 {
        db::audit(
            &state.db,
            ACTOR,
            "deferred",
            None,
            None,
            None,
            Some(json!({
                "count": report.deferred_indexing,
                "reason": "indexing_in_progress",
            })),
        )
        .await;
    }
    if report.deferred_rate > 0 {
        db::audit(
            &state.db,
            ACTOR,
            "deferred",
            None,
            None,
            None,
            Some(json!({ "count": report.deferred_rate, "reason": "rate_limited" })),
        )
        .await;
    }

    metrics::counter!("ovis_prune_deleted_total").increment(deleted_this_cycle as u64);
    Ok(())
}

/// Cascade one claimed document and record the honest outcome.
///
/// Every deletion is preceded by a trash snapshot, and the snapshot shares the
/// deletion's transaction (see [`ovis_core::db::trash::trash_and_delete`]). A
/// document that cannot be snapshotted is **not** deleted: the whole point of
/// the trash is that the last step of pruning stopped being irreversible, and
/// a silent fallback to unrecoverable deletion would give that back.
async fn delete_one(
    state: &AppState,
    index: &str,
    row: &PruneCandidateItem,
) -> Result<(), &'static str> {
    match trash_delete(state, index, row).await {
        Ok(outcome) => {
            let outcome_json = json!({
                "chunks_deleted": outcome.chunks_deleted,
                "index_cleanup_pending": outcome.index_cleanup_pending,
                "recrawl_risk": outcome.recrawl_risk,
                "trashed": true,
                "snapshot_bytes": outcome.snapshot_bytes,
                "restorable_until": outcome.restorable_until,
            });
            let _ = db::mark_deleted(&state.db, row.id, outcome_json.clone()).await;
            if row.remember {
                remember_exclusion(state, row).await;
            }
            db::audit(
                &state.db,
                ACTOR,
                "deleted",
                Some(&row.document_id),
                row.scan_id,
                Some(row.id),
                Some(outcome_json),
            )
            .await;
            Ok(())
        }
        Err(ovis_core::CoreError::NotFound { .. }) => {
            // The document vanished underneath us (connector delete, manual
            // delete). The index may still hold chunks; queue cleanup.
            let _ = ovis_core::db::pending_deletes::enqueue(
                &state.db,
                &row.document_id,
                "document row already gone when the reaper ran",
            )
            .await;
            let outcome_json = json!({
                "chunks_deleted": null,
                "index_cleanup_pending": true,
                "already_gone": true,
            });
            let _ = db::mark_deleted(&state.db, row.id, outcome_json.clone()).await;
            if row.remember {
                remember_exclusion(state, row).await;
            }
            db::audit(
                &state.db,
                ACTOR,
                "deleted",
                Some(&row.document_id),
                row.scan_id,
                Some(row.id),
                Some(outcome_json),
            )
            .await;
            Ok(())
        }
        Err(err) => {
            // The cascade failed and rolled back — the document is intact.
            // Unclaim so the next cycle retries.
            let _ = db::unclaim_deletion(&state.db, row.id).await;
            db::audit(
                &state.db,
                ACTOR,
                "delete_failed",
                Some(&row.document_id),
                row.scan_id,
                Some(row.id),
                Some(json!({ "error": err.to_string() })),
            )
            .await;
            tracing::warn!(document_id = %row.document_id, error = %err, "reaper delete failed; will retry");
            Err("DELETE_FAILED")
        }
    }
}

/// What one trashed deletion produced.
struct TrashedOutcome {
    chunks_deleted: u64,
    index_cleanup_pending: bool,
    recrawl_risk: bool,
    snapshot_bytes: i64,
    restorable_until: chrono::DateTime<Utc>,
}

/// Snapshot, then delete, then clear the index.
///
/// Ordering is the safety property: the OpenSearch read happens *before* the
/// transaction (so a failure aborts with the document intact), the snapshot
/// and the Postgres cascade commit together, and the index delete happens
/// last — where a failure is ordinary cleanup debt rather than lost content,
/// because the chunk bodies are inside the snapshot.
async fn trash_delete(
    state: &AppState,
    index: &str,
    row: &PruneCandidateItem,
) -> Result<TrashedOutcome, ovis_core::CoreError> {
    let recrawl_risk = row.recrawl_risk;
    let mut snapshot = trash::capture(
        &state.db,
        &state.os,
        index,
        &row.document_id,
        state.cfg.trash_keep_vectors,
    )
    .await?;

    // Staging sets `hidden` on the way to deletion, so a snapshot taken at
    // delete time records the *pruning process*, not the document. Restoring
    // that verbatim would hand back a document invisible to search, which is
    // not what "put it back" means. `prev_hidden` is the flag the document
    // carried before pruning touched it, and that is what the snapshot keeps.
    if let Some(prev_hidden) = row.prev_hidden {
        if let Some(document) = snapshot.document.as_object_mut() {
            document.insert("hidden".into(), serde_json::Value::Bool(prev_hidden));
        }
    }

    let provenance = trash::TrashProvenance {
        candidate_id: Some(row.id),
        policy_hash: None,
        reasons: serde_json::to_value(&row.reasons).ok(),
        deleted_by: ACTOR.to_string(),
    };
    let snapshot_bytes = trash::trash_and_delete(
        &state.db,
        &snapshot,
        &provenance,
        state.cfg.trash_retention_days,
    )
    .await?;

    let (chunks_deleted, index_cleanup_pending) =
        match state.os.delete_document_chunks(index, &row.document_id).await {
            Ok(n) => (n, false),
            Err(err) => {
                tracing::warn!(
                    document_id = %row.document_id,
                    error = %err,
                    "document trashed and removed from Postgres, but index cleanup failed; \
                     queued for retry"
                );
                let _ = ovis_core::db::pending_deletes::enqueue(
                    &state.db,
                    &row.document_id,
                    &err.to_string(),
                )
                .await;
                (0, true)
            }
        };

    Ok(TrashedOutcome {
        chunks_deleted,
        index_cleanup_pending,
        recrawl_risk,
        snapshot_bytes,
        restorable_until: Utc::now() + ChronoDuration::days(state.cfg.trash_retention_days),
    })
}

/// Drop snapshots whose retention has run out. This is the only irreversible
/// step in the whole pruning system, and it is deliberately the slowest: it
/// runs after everything else in the cycle, in bounded batches, and never
/// touches a held snapshot.
async fn purge_expired_trash(state: &AppState, report: &mut CycleReport) -> Result<(), AppError> {
    let due = trash::due_for_purge(&state.db, state.cfg.trash_purge_batch_size).await?;
    if due.is_empty() {
        return Ok(());
    }
    let purged = trash::purge(&state.db, &due).await?;
    report.purged = purged as i64;
    db::audit(
        &state.db,
        ACTOR,
        "trash_purged",
        None,
        None,
        None,
        Some(json!({
            "count": purged,
            "retention_days": state.cfg.trash_retention_days,
            "reason": "retention_elapsed",
        })),
    )
    .await;
    tracing::info!(
        purged,
        retention_days = state.cfg.trash_retention_days,
        "purged expired trash snapshots"
    );
    Ok(())
}

async fn remember_exclusion(state: &AppState, row: &PruneCandidateItem) {
    let note = row
        .reasons
        .first()
        .map(|r| format!("{}/{}", r.detector, r.code));
    if let Err(err) = db::add_exclusion(
        &state.db,
        &row.document_id,
        "deleted_with_remember",
        note.as_deref(),
    )
    .await
    {
        tracing::error!(document_id = %row.document_id, error = %err, "failed to record a prune exclusion");
    }
}

/// Previously-deleted, remembered documents the crawler brought back are
/// **staged** — hidden, full grace period, normal lifecycle. Automation never
/// skips the waiting room.
async fn restage_recrawled(state: &AppState, report: &mut CycleReport) -> Result<(), AppError> {
    let recrawled = db::recrawled_exclusions(&state.db, RESTAGE_PER_CYCLE).await?;
    if recrawled.is_empty() {
        return Ok(());
    }
    let expires_at = prune::grace_deadline(state, Utc::now());

    for document_id in recrawled {
        let Some(doc) = db::scan_document_row(&state.db, &document_id).await? else {
            continue; // vanished again between the query and now
        };
        let recrawl_risk = doc
            .cc_pair_status
            .as_deref()
            .map(|s| ["ACTIVE", "INITIAL_INDEXING"].contains(&s))
            .unwrap_or(false);
        let hit = db::DetectorHit {
            document_id: document_id.clone(),
            reasons: vec![PruneReason {
                detector: "recrawl".into(),
                code: "recrawled_after_prune".into(),
                detail: "previously pruned with remember=true; the crawler brought it back"
                    .into(),
                confidence: 1.0,
                evidence: json!({ "exclusion_reason": "deleted_with_remember" }),
            }],
            connector_id: doc.connector_id,
            cc_pair_id: doc.cc_pair_id,
            chunk_count: doc.chunk_count,
            recrawl_risk,
        };
        let candidate_id = match db::insert_restage_candidate(&state.db, &hit).await {
            Ok(id) => id,
            Err(err) => {
                // A concurrent open row (unique index) is fine — someone beat
                // us to it.
                tracing::debug!(document_id = %document_id, error = %err, "restage insert skipped");
                continue;
            }
        };
        let Some(row) = db::get_candidate(&state.db, candidate_id).await? else {
            continue;
        };
        match prune::stage_one(state, &row, expires_at, ACTOR).await {
            Ok((via, prev_hidden)) => {
                // Keep remember set: if this copy is deleted too, the cycle
                // repeats — visible in the audit trail each time.
                let _ = db::set_remember(&state.db, candidate_id, true).await;
                report.restaged += 1;
                db::audit(
                    &state.db,
                    ACTOR,
                    "restaged_recrawled",
                    Some(&document_id),
                    None,
                    Some(candidate_id),
                    Some(json!({
                        "prev_hidden": prev_hidden,
                        "stage_expires_at": expires_at,
                        "via": via,
                    })),
                )
                .await;
            }
            Err(err) => {
                tracing::warn!(document_id = %document_id, error = %err.log_detail(), "recrawl re-stage failed");
            }
        }
    }
    Ok(())
}

async fn update_metrics(state: &AppState, report: &CycleReport) {
    if let Ok(counts) = db::state_counts(&state.db).await {
        metrics::gauge!("ovis_prune_candidates").set(counts.candidates as f64);
        metrics::gauge!("ovis_prune_staged").set(counts.staged as f64);
    }
    metrics::gauge!("ovis_prune_deferred")
        .set((report.deferred_indexing + report.deferred_rate) as f64);
    metrics::gauge!("ovis_prune_halted").set(if report.halted { 1.0 } else { 0.0 });
}

#[cfg(test)]
mod tests {
    /// The acceptance checklist's grep-level assertion, executable: no prune
    /// code path outside this module reaches the delete cascade. Staging,
    /// scanning, routing — none of them may delete a document.
    #[test]
    fn only_the_reaper_touches_the_delete_cascade() {
        let sources = [
            ("services/prune.rs", include_str!("prune.rs")),
            ("services/prune_scan.rs", include_str!("prune_scan.rs")),
            ("routes/prune.rs", include_str!("../routes/prune.rs")),
        ];
        for (name, source) in sources {
            for forbidden in [
                "delete_document_cascading",
                "delete_document_pg_only",
                "batch_delete",
                "delete_chunks_for",
                "delete_document_chunks",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{name} references {forbidden}; hard deletion belongs to the reaper alone"
                );
            }
        }
    }

    /// This module never touches the tables the landmines protect. The
    /// forbidden strings are assembled at runtime so the test cannot match
    /// its own source.
    #[test]
    fn the_reaper_never_writes_onyx_control_tables() {
        let source = include_str!("prune_reaper.rs");
        let writes = ["UPDATE", "INSERT INTO", "DELETE FROM"];
        let protected = ["index_attempt", "search_settings", "connector", ""];
        for write in writes {
            for table in protected {
                let forbidden = format!("{write} public.{table}");
                assert!(
                    !source.contains(&forbidden),
                    "the reaper must not run '{forbidden}'"
                );
            }
        }
    }
}
