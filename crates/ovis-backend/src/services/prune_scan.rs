//! The scan job runner: one background task, one scan at a time, keyset
//! checkpoints, resume after restart, cancel between pages.
//!
//! A scan **never mutates documents** — its only writes are `ovis.*` rows
//! (candidates, signatures, its own progress). Phases, in order:
//!
//! 1. `documents` — one keyset walk over the scope. Row-level detectors
//!    (thin/stub, stale, URL rules) read the row; tag rules read a per-page
//!    tag batch; content detectors (language, thin-words, near-duplicate
//!    signatures) fetch chunk text from the index — never source URLs.
//!    `examined / total` progress comes from this walk.
//! 2. `exact` — content-hash groups, pure Postgres, group-keyset.
//! 3. `near_pairs` — LSH band-bucket collisions over the persisted
//!    signatures, verified pairwise by estimated Jaccard.
//!
//! After a *completed* scan, open candidates the scan no longer flags are
//! closed with `resolved_reason: no_longer_matches`.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::Utc;
use ovis_core::api_types::{PruneReason, PruneScanItem};
use ovis_core::db::prune as db;
use ovis_core::db::prune::{DetectorHit, ScanDocRow, UpsertOutcome};
use ovis_prune::{content, MinHashDedupEngine, PreferKeepPolicy, PruneConfig};
use serde_json::{json, Value};

use crate::services::prune::CompiledRule;
use crate::state::AppState;
use ovis_core::db::profile as profile_db;
use ovis_core::db::profile::DocProfile;
use ovis_prune::{quality, urlkey};

const RECRAWLING_STATES: [&str; 2] = ["ACTIVE", "INITIAL_INDEXING"];

/// How many duplicate-hash groups one exact-phase page processes.
const EXACT_GROUP_PAGE: i64 = 200;

/// Chunks fetched per document for the content detectors. Covers the full
/// text of nearly every web page; recorded in reason evidence when it clips.
const CONTENT_CHUNK_CAP: i64 = 20;

/// Chunks examined for language detection.
const LANGUAGE_CHUNKS: usize = 2;

/// Band buckets per near-pair page.
const NEAR_BUCKET_PAGE: i64 = 200;

/// A band bucket larger than this is boilerplate gravity, not a duplicate
/// group; it is skipped and counted honestly in `oversized_buckets`.
const NEAR_BUCKET_CAP: i64 = 200;

/// Consecutive content-fetch failures that fail the scan: a dead OpenSearch
/// must stop the walk loudly, not produce a silently thinner candidate set.
const MAX_CONSECUTIVE_CONTENT_ERRORS: u32 = 25;

pub fn spawn_scan_runner(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = state.prune.scan_wake.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            }
            run_next_scan(&state).await;
        }
    });
}

/// Run the next queued (or resumable running) scan to completion. Returns
/// whether one was found. Public so tests can drive scans synchronously
/// instead of waiting on the background task.
pub async fn run_next_scan(state: &AppState) -> bool {
    match db::next_scan_to_run(&state.db).await {
        Ok(Some(scan)) => {
            run_scan(state, scan).await;
            true
        }
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(error = %err, "prune scan poll failed");
            false
        }
    }
}

/// Counters a scan accumulates. All server truths, persisted in the scan row.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanStats {
    pub candidates_new: i64,
    pub candidates_updated: i64,
    pub left_alone: i64,
    pub excluded_skipped: i64,
    pub candidates_closed: i64,
    pub stub_hits: i64,
    pub thin_content_hits: i64,
    pub dup_groups: i64,
    pub dup_members: i64,
    pub url_rule_hits: i64,
    pub tag_rule_hits: i64,
    pub stale_hits: i64,
    pub lang_hits: i64,
    pub content_docs_fetched: i64,
    pub content_errors: i64,
    pub signatures_written: i64,
    pub signatures_reused: i64,
    pub near_buckets: i64,
    pub oversized_buckets: i64,
    pub near_pairs_verified: i64,
    pub near_hits: i64,
    // v2 counters
    pub profiles_written: i64,
    pub quality_measured: i64,
    pub quality_hits: i64,
    pub asset_hits: i64,
    pub url_variant_groups: i64,
    pub url_variant_hits: i64,
    pub pairs_stored: i64,
}

impl ScanStats {
    fn record(&mut self, outcome: UpsertOutcome) {
        match outcome {
            UpsertOutcome::Inserted => self.candidates_new += 1,
            UpsertOutcome::Updated => self.candidates_updated += 1,
            UpsertOutcome::LeftAlone => self.left_alone += 1,
            UpsertOutcome::Excluded => self.excluded_skipped += 1,
        }
    }

    fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

/// The resume cursor persisted between pages.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct Checkpoint {
    #[serde(default)]
    done: Vec<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    phase: Option<String>,
    /// Errors survived so far. A transient failure (a LAN blip mid-corpus)
    /// resumes from the checkpoint; only repeated failure is terminal.
    #[serde(default)]
    retries: u32,
}

/// How many times a scan may error and resume before it is marked failed.
const MAX_SCAN_RETRIES: u32 = 5;

enum ScanEnd {
    Done,
    Cancelled,
}

async fn run_scan(state: &AppState, scan: PruneScanItem) {
    if !matches!(db::scan_mark_running(&state.db, scan.id).await, Ok(true)) {
        return;
    }

    let resumed = scan.status == "running";
    db::audit(
        &state.db,
        "scan",
        if resumed {
            "scan_resumed"
        } else {
            "scan_started"
        },
        None,
        Some(scan.id),
        None,
        Some(json!({ "detectors": scan.detectors, "scope": scan.scope })),
    )
    .await;

    match execute(state, &scan).await {
        Ok((ScanEnd::Done, mut stats)) => {
            // Scoped by this scan's id, so a resumed scan keeps the candidates
            // its earlier pass wrote and nothing depends on comparing clocks.
            match db::close_stale_candidates(&state.db, scan.id, &scan.detectors, &scan.scope).await
            {
                Ok(closed) => {
                    stats.candidates_closed = closed.len() as i64;
                    if !closed.is_empty() {
                        db::audit(
                            &state.db,
                            "scan",
                            "candidates_closed",
                            None,
                            Some(scan.id),
                            None,
                            Some(json!({
                                "count": closed.len(),
                                "reason": "no_longer_matches",
                            })),
                        )
                        .await;
                    }
                }
                Err(err) => {
                    tracing::warn!(scan_id = scan.id, error = %err, "closing stale candidates failed");
                }
            }
            let _ = db::scan_finish(&state.db, scan.id, "done", None, &stats.to_json()).await;
            db::audit(
                &state.db,
                "scan",
                "scan_finished",
                None,
                Some(scan.id),
                None,
                Some(stats.to_json()),
            )
            .await;
            tracing::info!(scan_id = scan.id, stats = %stats.to_json(), "prune scan finished");
        }
        Ok((ScanEnd::Cancelled, stats)) => {
            let _ = db::scan_finish(&state.db, scan.id, "cancelled", None, &stats.to_json()).await;
            db::audit(
                &state.db,
                "scan",
                "scan_cancelled_stopped",
                None,
                Some(scan.id),
                None,
                None,
            )
            .await;
            tracing::info!(
                scan_id = scan.id,
                "prune scan stopped at a cancellation checkpoint"
            );
        }
        Err(err) if is_config_too_new(&err) => {
            // Another, newer instance queued this scan with configuration this
            // build does not understand. Retrying will never help — but a
            // newer instance polling the same database *will* succeed, so the
            // scan is left exactly as it is, without consuming a retry.
            //
            // Observed live: a v0.3.0 container and a newer build shared one
            // database during a rolling upgrade, and the old one burned a
            // retry every poll. Marking the scan failed there would blame the
            // scan for a deployment state.
            db::audit(
                &state.db,
                "scan",
                "scan_deferred_version",
                None,
                Some(scan.id),
                None,
                Some(json!({
                    "error": err.log_detail(),
                    "reason": "config_snapshot_from_a_newer_build",
                })),
            )
            .await;
            tracing::warn!(
                scan_id = scan.id,
                error = %err.log_detail(),
                "this build cannot read the scan's configuration; leaving it for a newer \
                 instance rather than failing it"
            );
        }
        Err(err) => {
            let detail = err.log_detail();
            // Stats and checkpoint were persisted page by page; never wipe
            // them on failure — they are the evidence and the resume point.
            let (mut checkpoint, stats) = match (
                db::scan_checkpoint_value(&state.db, scan.id).await,
                db::get_scan(&state.db, scan.id).await,
            ) {
                (Ok(value), Ok(Some(row))) => (
                    value
                        .and_then(|v| serde_json::from_value::<Checkpoint>(v).ok())
                        .unwrap_or_default(),
                    row.stats,
                ),
                _ => (Checkpoint::default(), json!({})),
            };
            checkpoint.retries += 1;

            if checkpoint.retries <= MAX_SCAN_RETRIES {
                // Leave the scan `running`: the runner's next poll resumes it
                // from the checkpoint. A transient blip costs one page, not
                // an hours-long scan.
                let _ = db::scan_checkpoint(
                    &state.db,
                    scan.id,
                    scan.examined,
                    scan.total,
                    &serde_json::to_value(&checkpoint).unwrap_or_default(),
                    &stats,
                )
                .await;
                db::audit(
                    &state.db,
                    "scan",
                    "scan_retrying",
                    None,
                    Some(scan.id),
                    None,
                    Some(json!({
                        "error": detail,
                        "retry": checkpoint.retries,
                        "max_retries": MAX_SCAN_RETRIES,
                    })),
                )
                .await;
                tracing::warn!(
                    scan_id = scan.id,
                    retry = checkpoint.retries,
                    error = %detail,
                    "prune scan hit an error; will resume from the checkpoint"
                );
                return;
            }

            let _ = db::scan_finish(&state.db, scan.id, "failed", Some(&detail), &stats).await;
            db::audit(
                &state.db,
                "scan",
                "scan_failed",
                None,
                Some(scan.id),
                None,
                Some(json!({ "error": detail, "retries": checkpoint.retries })),
            )
            .await;
            tracing::error!(scan_id = scan.id, error = %detail, "prune scan failed");
        }
    }
}

/// Whether a scan failed because its stored configuration was written by a
/// build that knows fields this one does not.
///
/// The snapshot is deserialized with `deny_unknown_fields`, deliberately: a
/// scan must run under exactly the configuration it was queued with, and
/// silently dropping a setting it does not understand would produce a scan
/// that quietly did less than it was asked to. The right response is to leave
/// the work for a build that can do it.
fn is_config_too_new(err: &crate::error::AppError) -> bool {
    matches!(err, crate::error::AppError::BadRequest(detail)
        if detail.contains("scan config snapshot") && detail.contains("unknown field"))
}

/// The scan's configuration comes from its own snapshot — the config it was
/// queued under — never from the current rules table. A threshold change
/// after queueing is a different scan.
fn config_from_snapshot(
    snapshot: &Value,
) -> Result<(PruneConfig, Vec<CompiledRule>, Vec<CompiledRule>), crate::error::AppError> {
    let config: PruneConfig = serde_json::from_value(
        snapshot.get("config").cloned().unwrap_or(Value::Null),
    )
    .map_err(|e| crate::error::AppError::BadRequest(format!("scan config snapshot: {e}")))?;
    let url_rules = compiled_from_snapshot(snapshot, "url_rules")?;
    let tag_rules = compiled_from_snapshot(snapshot, "tag_rules")?;
    Ok((config, url_rules, tag_rules))
}

fn compiled_from_snapshot(
    snapshot: &Value,
    key: &str,
) -> Result<Vec<CompiledRule>, crate::error::AppError> {
    let raw = snapshot.get(key).cloned().unwrap_or(Value::Null);
    let mut rules = Vec::new();
    if let Some(entries) = raw.as_array() {
        for entry in entries {
            let name = entry.get("name").and_then(Value::as_str).unwrap_or("rule");
            let pattern = entry
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let confidence = entry
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.8) as f32;
            let regex = regex::Regex::new(pattern).map_err(|e| {
                crate::error::AppError::BadRequest(format!(
                    "snapshot rule '{name}' pattern does not compile: {e}"
                ))
            })?;
            rules.push(CompiledRule {
                name: name.to_string(),
                pattern: pattern.to_string(),
                regex,
                confidence,
            });
        }
    }
    Ok(rules)
}

/// Everything one scan run needs, resolved once.
struct ScanCtx<'a> {
    state: &'a AppState,
    scan: &'a PruneScanItem,
    config: PruneConfig,
    url_rules: Vec<CompiledRule>,
    tag_rules: Vec<CompiledRule>,
    engine: Option<MinHashDedupEngine>,
    minhash_config_hash: String,
    /// The scan's own config hash, stamped onto every profile so a later scan
    /// can tell which measurements are stale.
    config_hash: String,
}

impl ScanCtx<'_> {
    fn wants(&self, name: &str) -> bool {
        self.scan.detectors.iter().any(|d| d == name)
    }

    fn wants_content(&self) -> bool {
        self.wants("near_duplicate") || self.wants("language") || self.wants("quality")
    }
}

async fn execute(
    state: &AppState,
    scan: &PruneScanItem,
) -> Result<(ScanEnd, ScanStats), crate::error::AppError> {
    let snapshot = db::scan_config_snapshot(&state.db, scan.id).await?;
    let (config, url_rules, tag_rules) = config_from_snapshot(&snapshot)?;

    let wants_near = scan.detectors.iter().any(|d| d == "near_duplicate");
    let engine = wants_near.then(|| MinHashDedupEngine::new(config.dedup.minhash.clone()));
    let minhash_config_hash = format!(
        "np{}-k{}-b{}",
        config.dedup.minhash.num_perm,
        config.dedup.minhash.shingle_size,
        config.dedup.minhash.bands.unwrap_or(16),
    );
    let ctx = ScanCtx {
        state,
        scan,
        config,
        url_rules,
        tag_rules,
        engine,
        minhash_config_hash,
        config_hash: scan.config_hash.clone(),
    };

    // Restore checkpoint + stats from the scan row (resume case).
    let fresh = db::get_scan(&state.db, scan.id).await?.ok_or_else(|| {
        crate::error::AppError::NotFound {
            what: "prune scan",
            id: scan.id.to_string(),
        }
    })?;
    let mut stats: ScanStats = serde_json::from_value(fresh.stats.clone()).unwrap_or_default();
    let mut checkpoint: Checkpoint = match db::scan_checkpoint_value(&state.db, scan.id).await? {
        Some(value) => serde_json::from_value(value).unwrap_or_default(),
        None => Checkpoint::default(),
    };
    // Progress resets the retry budget: five blips across an hours-long scan
    // must not accumulate into a terminal failure.
    checkpoint.retries = 0;
    let mut examined = fresh.examined;

    let total = db::scan_scope_total(&state.db, &scan.scope).await?;
    let done = |checkpoint: &Checkpoint, phase: &str| checkpoint.done.iter().any(|p| p == phase);

    // A MinHash parameter change makes the persisted signatures incomparable.
    if ctx.wants("near_duplicate") && !done(&checkpoint, "documents") {
        let dropped =
            db::minhash_reset_if_config_changed(&state.db, &ctx.minhash_config_hash).await?;
        if dropped > 0 {
            tracing::info!(
                dropped,
                "minhash parameters changed; the signature store was reset"
            );
        }
    }

    // ---- phase 1: the document walk ----
    let walk_wanted = [
        "thin",
        "url_rule",
        "tag_rule",
        "stale",
        "language",
        "near_duplicate",
        "quality",
        "url_junk",
    ]
    .iter()
    .any(|d| ctx.wants(d));
    if walk_wanted && !done(&checkpoint, "documents") {
        let mut cursor = if checkpoint.phase.as_deref() == Some("documents") {
            checkpoint.cursor.clone()
        } else {
            None
        };
        let mut consecutive_content_errors: u32 = 0;
        loop {
            let page = db::scan_documents_page(
                &state.db,
                &scan.scope,
                cursor.as_deref(),
                state.cfg.prune_scan_page_size,
            )
            .await?;
            if page.is_empty() {
                break;
            }

            // Tag rules read one batch per page.
            let page_tags: HashMap<String, Vec<(String, String)>> =
                if ctx.wants("tag_rule") && !ctx.tag_rules.is_empty() {
                    let ids: Vec<String> = page.iter().map(|d| d.id.clone()).collect();
                    let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
                    for (doc_id, key, value) in db::tags_for_documents(&state.db, &ids).await? {
                        map.entry(doc_id).or_default().push((key, value));
                    }
                    map
                } else {
                    HashMap::new()
                };

            // Accumulated per page, flushed in one round trip each. The v1
            // loop wrote every candidate individually, which is what made the
            // full-corpus scan LAN-latency-bound rather than database-bound.
            let mut page_profiles: Vec<DocProfile> = Vec::with_capacity(page.len());
            let mut page_hits: Vec<DetectorHit> = Vec::new();

            // One `_msearch` for the whole page instead of one query per
            // document: measured 137 docs/s per-document against gamma, which
            // is 3.5 hours for the full corpus.
            let page_content = if ctx.wants_content() {
                fetch_page_content(&ctx, &page, &mut stats).await?
            } else {
                HashMap::new()
            };

            for doc in &page {
                let mut reasons: Vec<PruneReason> = Vec::new();
                let mut profile = base_profile(doc, &ctx.config_hash);

                // URL shape is free — no text, no index round trip — so it is
                // measured for every document regardless of which detectors
                // were asked for.
                if let Some(reason) = url_signals(doc, &ctx.config, &mut profile) {
                    if ctx.wants("url_junk") {
                        stats.asset_hits += 1;
                        reasons.push(reason);
                    }
                }

                if ctx.wants("thin") {
                    if let Some(reason) = stub_reason(doc, &ctx.config) {
                        stats.stub_hits += 1;
                        reasons.push(reason);
                    }
                }
                if ctx.wants("stale") {
                    if let Some(reason) = stale_reason(doc, &ctx.config) {
                        stats.stale_hits += 1;
                        reasons.push(reason);
                    }
                }
                if ctx.wants("url_rule") {
                    for rule in &ctx.url_rules {
                        if rule.regex.is_match(&doc.id) {
                            stats.url_rule_hits += 1;
                            reasons.push(url_rule_reason(doc, rule));
                        }
                    }
                }
                if ctx.wants("tag_rule") {
                    if let Some(tags) = page_tags.get(&doc.id) {
                        for rule in &ctx.tag_rules {
                            if let Some((key, value)) = tags.iter().find(|(k, v)| {
                                let kv = format!("{k}={v}");
                                rule.regex.is_match(&kv) || rule.regex.is_match(v)
                            }) {
                                stats.tag_rule_hits += 1;
                                reasons.push(tag_rule_reason(rule, key, value));
                            }
                        }
                    }
                }

                // Content pass: one fetch feeds language, thin-words and the
                // near-duplicate signature. 0-chunk documents have nothing to
                // fetch (the stub detector owns them).
                if ctx.wants_content() && doc.chunk_count != Some(0) {
                    match content_pass(
                        &ctx,
                        doc,
                        page_content.get(&doc.id),
                        &mut stats,
                        &mut profile,
                    )
                    .await
                    {
                        Ok(mut content_reasons) => {
                            consecutive_content_errors = 0;
                            reasons.append(&mut content_reasons);
                        }
                        Err(err) => {
                            stats.content_errors += 1;
                            consecutive_content_errors += 1;
                            tracing::warn!(
                                document_id = %doc.id,
                                error = %err.log_detail(),
                                "content processing failed during scan"
                            );
                            if consecutive_content_errors >= MAX_CONSECUTIVE_CONTENT_ERRORS {
                                return Err(crate::error::AppError::UpstreamSearch(format!(
                                    "{consecutive_content_errors} consecutive content fetches \
                                     failed; the index is unreachable — the scan stops rather \
                                     than producing a silently thinner candidate set"
                                )));
                            }
                        }
                    }
                }

                page_profiles.push(profile);
                if !reasons.is_empty() {
                    page_hits.push(hit_from(doc, reasons));
                }
            }

            // A profile is written for every document examined, flagged or
            // not: policy has to be able to answer "what would a looser
            // setting catch?", and that means knowing about the documents a
            // scan decided were fine.
            match profile_db::upsert_profiles(&state.db, &page_profiles).await {
                Ok(written) => stats.profiles_written += written as i64,
                Err(err) => {
                    tracing::warn!(error = %err, "writing document profiles failed");
                }
            }
            for outcome in db::upsert_candidates(&state.db, Some(scan.id), &page_hits).await? {
                stats.record(outcome);
            }

            examined += page.len() as i64;
            cursor = page.last().map(|d| d.id.clone());
            checkpoint.phase = Some("documents".into());
            checkpoint.cursor = cursor.clone();
            let status = db::scan_checkpoint(
                &state.db,
                scan.id,
                examined,
                Some(total),
                &serde_json::to_value(&checkpoint).unwrap_or_default(),
                &stats.to_json(),
            )
            .await?;
            if status == "cancelled" {
                return Ok((ScanEnd::Cancelled, stats));
            }
            tokio::task::yield_now().await;
        }
        checkpoint.done.push("documents".into());
        checkpoint.cursor = None;
        checkpoint.phase = None;
    }

    // ---- phase 2: exact duplicates (content_hash groups) ----
    if ctx.wants("exact_duplicate") && !done(&checkpoint, "exact") {
        let mut cursor = if checkpoint.phase.as_deref() == Some("exact") {
            checkpoint.cursor.clone()
        } else {
            None
        };
        loop {
            let groups = db::duplicate_hash_groups_page(
                &state.db,
                &scan.scope,
                cursor.as_deref(),
                EXACT_GROUP_PAGE,
            )
            .await?;
            if groups.is_empty() {
                break;
            }
            let hashes: Vec<String> = groups.iter().map(|(h, _)| h.clone()).collect();
            let members = db::documents_for_hashes(&state.db, &scan.scope, &hashes).await?;

            let mut by_hash: BTreeMap<&str, Vec<&ScanDocRow>> = BTreeMap::new();
            for member in &members {
                if let Some(hash) = member.content_hash.as_deref() {
                    by_hash.entry(hash).or_default().push(member);
                }
            }

            let mut page_hits: Vec<DetectorHit> = Vec::new();
            let mut group_memberships: Vec<profile_db::DupMembership> = Vec::new();
            for (hash, group) in by_hash {
                if group.len() < 2 {
                    continue;
                }
                stats.dup_groups += 1;
                let cross_connector = spans_connectors(&group);
                let keeper = select_keeper(&group, ctx.config.dedup.prefer_keep);
                for member in &group {
                    // The keeper is recorded as a group member too: review
                    // needs to show the whole cluster, and policy excludes
                    // keepers by their role rather than by their absence.
                    group_memberships.push(profile_db::DupMembership {
                        document_id: member.id.clone(),
                        method: "hash".into(),
                        group_key: hash.to_string(),
                        group_size: group.len() as i32,
                        is_keeper: member.id == keeper.id,
                        cross_connector,
                    });
                    if member.id == keeper.id {
                        continue;
                    }
                    stats.dup_members += 1;
                    page_hits.push(exact_dup_hit(
                        member,
                        keeper,
                        hash,
                        group.len(),
                        ctx.config.dedup.prefer_keep,
                    ));
                }
            }
            // Every grouped document needs a profile row or policy cannot see
            // it; a scan asking only for `exact_duplicate` never runs the walk
            // that would otherwise have written one.
            let page_profiles: Vec<DocProfile> = members
                .iter()
                .map(|m| grouping_profile(m, &ctx.config_hash))
                .collect();
            if let Err(err) = profile_db::upsert_profiles(&state.db, &page_profiles).await {
                tracing::warn!(error = %err, "writing profiles for grouped documents failed");
            }
            if let Err(err) = profile_db::set_dup_groups(&state.db, &group_memberships).await {
                tracing::warn!(error = %err, "recording duplicate-group membership failed");
            }
            for outcome in db::upsert_candidates(&state.db, Some(scan.id), &page_hits).await? {
                stats.record(outcome);
            }

            cursor = groups.last().map(|(h, _)| h.clone());
            checkpoint.phase = Some("exact".into());
            checkpoint.cursor = cursor.clone();
            let status = db::scan_checkpoint(
                &state.db,
                scan.id,
                examined,
                Some(total),
                &serde_json::to_value(&checkpoint).unwrap_or_default(),
                &stats.to_json(),
            )
            .await?;
            if status == "cancelled" {
                return Ok((ScanEnd::Cancelled, stats));
            }
            tokio::task::yield_now().await;
        }
        checkpoint.done.push("exact".into());
        checkpoint.cursor = None;
        checkpoint.phase = None;
    }

    // ---- phase 2b: URL-variant groups ----
    //
    // Documents whose canonical URL matches are the same page reached by
    // different routes: tracking parameters, http/https, trailing slashes,
    // index filenames. Onyx's content_hash misses these whenever the two
    // crawls extracted even slightly different bytes, which a timestamp in a
    // footer is enough to cause.
    if ctx.wants("url_variant") && !done(&checkpoint, "url_variant") {
        let mut cursor = if checkpoint.phase.as_deref() == Some("url_variant") {
            checkpoint.cursor.clone()
        } else {
            None
        };
        loop {
            let groups =
                profile_db::canonical_url_groups(&state.db, cursor.as_deref(), EXACT_GROUP_PAGE)
                    .await?;
            if groups.is_empty() {
                break;
            }
            let keys: Vec<String> = groups.iter().map(|(k, _)| k.clone()).collect();
            let memberships = profile_db::documents_for_canonical_urls(&state.db, &keys).await?;

            let mut by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (key, doc_id) in memberships {
                by_key.entry(key).or_default().push(doc_id);
            }
            let all_ids: Vec<String> = by_key.values().flatten().cloned().collect();
            let rows = db::scan_documents_by_ids(&state.db, Some(&scan.scope), &all_ids).await?;
            let by_id: HashMap<&str, &ScanDocRow> =
                rows.iter().map(|d| (d.id.as_str(), d)).collect();

            let mut page_hits: Vec<DetectorHit> = Vec::new();
            let mut group_memberships: Vec<profile_db::DupMembership> = Vec::new();
            for (key, ids) in &by_key {
                let members: Vec<&ScanDocRow> = ids
                    .iter()
                    .filter_map(|id| by_id.get(id.as_str()).copied())
                    .collect();
                if members.len() < 2 {
                    continue;
                }
                stats.url_variant_groups += 1;
                let cross_connector = spans_connectors(&members);
                let keeper = select_keeper(&members, ctx.config.dedup.prefer_keep);
                for member in &members {
                    group_memberships.push(profile_db::DupMembership {
                        document_id: member.id.clone(),
                        method: "url".into(),
                        group_key: key.clone(),
                        group_size: members.len() as i32,
                        is_keeper: member.id == keeper.id,
                        cross_connector,
                    });
                    if member.id == keeper.id {
                        continue;
                    }
                    stats.url_variant_hits += 1;
                    page_hits.push(hit_from(
                        member,
                        vec![PruneReason {
                            detector: "url_junk".into(),
                            code: "url_variant_of".into(),
                            detail: format!(
                                "same canonical URL as {} (group of {}; keeper chosen by {})",
                                keeper.id,
                                members.len(),
                                policy_name(ctx.config.dedup.prefer_keep),
                            ),
                            confidence: ctx.config.url_junk.url_variant_confidence,
                            evidence: json!({
                                "kept": keeper.id,
                                "canonical_url": key,
                                "group_size": members.len(),
                                "policy": policy_name(ctx.config.dedup.prefer_keep),
                            }),
                        }],
                    ));
                }
            }
            let page_profiles: Vec<DocProfile> = rows
                .iter()
                .map(|d| grouping_profile(d, &ctx.config_hash))
                .collect();
            if let Err(err) = profile_db::upsert_profiles(&state.db, &page_profiles).await {
                tracing::warn!(error = %err, "writing profiles for grouped documents failed");
            }
            if let Err(err) = profile_db::set_dup_groups(&state.db, &group_memberships).await {
                tracing::warn!(error = %err, "recording URL-variant membership failed");
            }
            for outcome in db::upsert_candidates(&state.db, Some(scan.id), &page_hits).await? {
                stats.record(outcome);
            }

            cursor = groups.last().map(|(k, _)| k.clone());
            checkpoint.phase = Some("url_variant".into());
            checkpoint.cursor = cursor.clone();
            let status = db::scan_checkpoint(
                &state.db,
                scan.id,
                examined,
                Some(total),
                &serde_json::to_value(&checkpoint).unwrap_or_default(),
                &stats.to_json(),
            )
            .await?;
            if status == "cancelled" {
                return Ok((ScanEnd::Cancelled, stats));
            }
            tokio::task::yield_now().await;
        }
        checkpoint.done.push("url_variant".into());
        checkpoint.cursor = None;
        checkpoint.phase = None;
    }

    // ---- phase 3: near-duplicate pairs over the persisted signatures ----
    if ctx.wants("near_duplicate") && !done(&checkpoint, "near_pairs") {
        let mut cursor: Option<(i16, i64)> = if checkpoint.phase.as_deref() == Some("near_pairs") {
            checkpoint.cursor.as_deref().and_then(parse_band_cursor)
        } else {
            None
        };
        // Pairs already verified this scan (buckets overlap across bands).
        let mut seen_pairs: HashSet<(String, String)> = HashSet::new();

        loop {
            let buckets =
                db::minhash_collision_buckets(&state.db, cursor, NEAR_BUCKET_PAGE).await?;
            if buckets.is_empty() {
                break;
            }
            for (band, hash, member_count) in &buckets {
                stats.near_buckets += 1;
                if *member_count > NEAR_BUCKET_CAP {
                    stats.oversized_buckets += 1;
                    continue;
                }
                let members =
                    db::minhash_bucket_members(&state.db, *band, *hash, NEAR_BUCKET_CAP).await?;
                near_pairs_for_bucket(&ctx, &members, &mut seen_pairs, &mut stats).await?;
            }

            cursor = buckets.last().map(|(b, h, _)| (*b, *h));
            checkpoint.phase = Some("near_pairs".into());
            checkpoint.cursor = cursor.map(|(b, h)| format!("{b}:{h}"));
            let status = db::scan_checkpoint(
                &state.db,
                scan.id,
                examined,
                Some(total),
                &serde_json::to_value(&checkpoint).unwrap_or_default(),
                &stats.to_json(),
            )
            .await?;
            if status == "cancelled" {
                return Ok((ScanEnd::Cancelled, stats));
            }
            tokio::task::yield_now().await;
        }
        checkpoint.done.push("near_pairs".into());
        checkpoint.cursor = None;
        checkpoint.phase = None;
    }

    if !walk_wanted {
        examined = total;
    }
    let _ = db::scan_checkpoint(
        &state.db,
        scan.id,
        examined,
        Some(total),
        &serde_json::to_value(&checkpoint).unwrap_or_default(),
        &stats.to_json(),
    )
    .await?;

    Ok((ScanEnd::Done, stats))
}

fn parse_band_cursor(raw: &str) -> Option<(i16, i64)> {
    let (band, hash) = raw.split_once(':')?;
    Some((band.parse().ok()?, hash.parse().ok()?))
}

// ---------------------------------------------------------------------------
// Content pass
// ---------------------------------------------------------------------------

/// The measurements every document gets, whether or not anything flags it.
fn base_profile(doc: &ScanDocRow, config_hash: &str) -> DocProfile {
    DocProfile {
        document_id: doc.id.clone(),
        config_hash: Some(config_hash.to_string()),
        fingerprint: Some(fingerprint_of(doc)),
        connector_id: doc.connector_id,
        chunk_count: doc.chunk_count,
        content_hash: doc.content_hash.clone(),
        // Duplicate-group membership lives in `ovis.doc_dup_group`, written by
        // the two grouping phases; a document can be in one group per method.
        ..DocProfile::default()
    }
}

/// The bare profile a grouping phase writes for a document the walk never saw.
///
/// Policy evaluates bands over `ovis.doc_profile`, so a document with no row
/// there is invisible to it however many duplicate groups it belongs to — and
/// a scan asking only for `exact_duplicate` never runs the document walk. The
/// fingerprint is deliberately absent: it is the "already measured under this
/// config" marker, and setting it from a phase that measured almost nothing
/// would make a later scan skip the content pass it still owes this document.
fn grouping_profile(doc: &ScanDocRow, config_hash: &str) -> DocProfile {
    DocProfile {
        fingerprint: None,
        ..base_profile(doc, config_hash)
    }
}

/// URL-shape signals: canonical key, class, and the asset reason.
///
/// Writes into the profile unconditionally (it is free) and returns a reason
/// only when the document is an asset whose indexed text is a crawl artefact
/// rather than content — an image URL with one chunk saying
/// `photo.jpg (1200×800)`.
fn url_signals(
    doc: &ScanDocRow,
    config: &PruneConfig,
    profile: &mut DocProfile,
) -> Option<PruneReason> {
    let url = doc.link.as_deref().unwrap_or(&doc.id);
    profile.canonical_url = urlkey::canonical_key(url);
    profile.path_depth = Some(urlkey::path_depth(url) as i16);
    profile.has_query = Some(urlkey::has_query(url));
    let class = urlkey::classify(url);
    profile.url_class = Some(class.code().to_string());
    if config.url_junk.flag_archive_editions {
        profile.archive_of = urlkey::archive_edition_of(url);
    }

    let exempt = doc
        .connector_name
        .as_deref()
        .map(|name| config.url_junk.exempt_connectors.iter().any(|c| c == name))
        .unwrap_or(false);
    if exempt {
        return None;
    }

    let flaggable = if class.is_asset() {
        config.url_junk.flag_assets
    } else if class == urlkey::UrlClass::BinaryDocument {
        config.url_junk.flag_binary_documents
    } else {
        false
    };
    if !flaggable {
        return None;
    }
    // An asset with real extracted text (an OCR'd scan, a PDF-backed image) is
    // content; only the ones whose chunk count says "filename and dimensions"
    // are junk.
    let chunk_count = doc.chunk_count?;
    if chunk_count > config.url_junk.asset_max_chunks {
        return None;
    }

    Some(PruneReason {
        detector: "url_junk".into(),
        code: "asset_url".into(),
        detail: format!(
            "{} URL indexed as a page, with {chunk_count} chunk(s) of extracted text",
            class.code()
        ),
        confidence: config.url_junk.asset_confidence,
        evidence: json!({
            "url_class": class.code(),
            "chunk_count": chunk_count,
            "max_chunks": config.url_junk.asset_max_chunks,
        }),
    })
}

/// A document's fetched text, plus whether the chunk cap clipped it.
struct PageText {
    full_text: String,
    texts: Vec<String>,
    clipped: bool,
}

/// Fetch chunk text for a whole page in one request.
///
/// Documents with zero chunks are skipped — there is nothing to fetch, and the
/// stub detector owns them.
async fn fetch_page_content(
    ctx: &ScanCtx<'_>,
    page: &[ScanDocRow],
    stats: &mut ScanStats,
) -> Result<HashMap<String, PageText>, crate::error::AppError> {
    let ids: Vec<String> = page
        .iter()
        .filter(|d| d.chunk_count != Some(0))
        .map(|d| d.id.clone())
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let runtime = ctx.state.runtime();
    let results = ctx
        .state
        .os
        .document_chunks_batch(&runtime.index_name, &ids, CONTENT_CHUNK_CAP, true)
        .await?;
    stats.content_docs_fetched += results.len() as i64;

    let mut out = HashMap::with_capacity(results.len());
    for (id, (chunks, total_chunks)) in ids.into_iter().zip(results) {
        let texts: Vec<String> = chunks.into_iter().filter_map(|c| c.content).collect();
        if texts.is_empty() {
            continue;
        }
        let clipped = total_chunks > texts.len() as i64;
        out.insert(
            id,
            PageText {
                full_text: texts.join("\n\n"),
                texts,
                clipped,
            },
        );
    }
    Ok(out)
}

/// Feed every content detector from text the page fetch already retrieved.
async fn content_pass(
    ctx: &ScanCtx<'_>,
    doc: &ScanDocRow,
    content: Option<&PageText>,
    stats: &mut ScanStats,
    profile: &mut DocProfile,
) -> Result<Vec<PruneReason>, crate::error::AppError> {
    let mut reasons = Vec::new();

    // Signature reuse: unchanged content (same fingerprint) skips the fetch
    // entirely when only near-dup needs content.
    let fingerprint = fingerprint_of(doc);
    let mut need_signature = false;
    if ctx.wants("near_duplicate") {
        let existing = db::minhash_fingerprints(
            &ctx.state.db,
            &ctx.minhash_config_hash,
            std::slice::from_ref(&doc.id),
        )
        .await?;
        need_signature = existing
            .first()
            .map(|(_, fp)| fp != &fingerprint)
            .unwrap_or(true);
        if !need_signature {
            stats.signatures_reused += 1;
        }
    }
    let language_wanted = ctx.wants("language") && ctx.config.language.enabled && {
        // Per-connector opt-out, e.g. a site that legitimately hosts many
        // languages.
        doc.connector_name
            .as_deref()
            .and_then(|name| ctx.config.language.per_connector_overrides.get(name))
            .map(|o| o.enabled)
            .unwrap_or(true)
    };
    let thin_words_wanted = ctx.wants("thin");
    let quality_wanted = ctx.wants("quality") && {
        doc.connector_name
            .as_deref()
            .map(|name| {
                !ctx.config
                    .quality
                    .exempt_connectors
                    .iter()
                    .any(|c| c == name)
            })
            .unwrap_or(true)
    };

    if !need_signature && !language_wanted && !thin_words_wanted && !quality_wanted {
        return Ok(reasons);
    }

    // The page fetch already retrieved this; a document missing from it simply
    // had no extractable text.
    let Some(content) = content else {
        return Ok(reasons);
    };
    let texts: Vec<&str> = content.texts.iter().map(String::as_str).collect();
    let full_text = &content.full_text;
    let clipped = content.clipped;

    if language_wanted {
        let sample: Vec<&str> = texts.iter().take(LANGUAGE_CHUNKS).copied().collect();
        if let Some(verdict) = content::detect_language(&sample, &ctx.config.language) {
            if !verdict.allowed {
                stats.lang_hits += 1;
                let mixed = verdict.mixed_with.is_some();
                let confidence = if mixed {
                    (verdict.confidence * 0.7) as f32
                } else {
                    verdict.confidence as f32
                };
                reasons.push(PruneReason {
                    detector: "language".into(),
                    code: "lang_not_allowed".into(),
                    detail: match &verdict.mixed_with {
                        Some(other) => format!(
                            "detected: {} ({:.2}), mixed with {other}; allowed: {}",
                            verdict.detected,
                            verdict.confidence,
                            ctx.config.language.allowed.join(", ")
                        ),
                        None => format!(
                            "detected: {} ({:.2}); allowed: {}",
                            verdict.detected,
                            verdict.confidence,
                            ctx.config.language.allowed.join(", ")
                        ),
                    },
                    confidence,
                    evidence: json!({
                        "detected": verdict.detected,
                        "detector_confidence": verdict.confidence,
                        "sample_len": verdict.sample_len,
                        "chunks_checked": sample.len(),
                        "mixed_with": verdict.mixed_with,
                        "allowed": ctx.config.language.allowed,
                    }),
                });
            }
        }
    }

    if quality_wanted {
        let metrics = quality::measure(full_text);
        let failures = quality::evaluate(&metrics, &ctx.config.quality);
        stats.quality_measured += 1;
        profile.word_count = Some(metrics.word_count as i32);
        profile.quality_metrics = serde_json::to_value(&metrics).ok();
        profile.quality_gates = Some(failures.iter().map(|g| g.code().to_string()).collect());
        profile.quality_fail_count = failures.len() as i16;
        profile.quality_families = quality::families_failed(&failures) as i16;

        // Measured always; flagged only when the failures clear both bars.
        // A document clipped at the chunk cap is measured on a prefix, so its
        // length-shaped gates would be measuring the cap rather than the page.
        if !clipped && quality::is_candidate(&failures, &ctx.config.quality) {
            stats.quality_hits += 1;
            let explanations: Vec<String> = failures
                .iter()
                .map(|g| g.explain(&metrics, &ctx.config.quality))
                .collect();
            reasons.push(PruneReason {
                detector: "quality".into(),
                code: "low_quality_text".into(),
                detail: format!(
                    "{} quality gates failed across {} categories: {}",
                    failures.len(),
                    quality::families_failed(&failures),
                    explanations.join("; ")
                ),
                confidence: quality::confidence(&failures, &ctx.config.quality),
                evidence: json!({
                    "gates": failures.iter().map(|g| g.code()).collect::<Vec<_>>(),
                    "families": quality::families_failed(&failures),
                    "explanations": explanations,
                    "word_count": metrics.word_count,
                    "min_failures": ctx.config.quality.min_failures,
                    "min_families": ctx.config.quality.min_families,
                }),
            });
        }
    }

    if thin_words_wanted && !clipped {
        let words = content::word_count(full_text);
        if words > 0 && words < ctx.config.thin.min_words {
            stats.thin_content_hits += 1;
            reasons.push(PruneReason {
                detector: "thin".into(),
                code: "thin_content".into(),
                detail: format!(
                    "{words} words of content (threshold: {})",
                    ctx.config.thin.min_words
                ),
                confidence: ctx.config.thin.short_confidence,
                evidence: json!({
                    "words": words,
                    "min_words": ctx.config.thin.min_words,
                    "chunks": texts.len(),
                }),
            });
        }
    }

    if need_signature {
        if let Some(engine) = &ctx.engine {
            let sig = engine.signature_for_text(full_text);
            let bands: Vec<i64> = engine
                .band_hashes(&sig)
                .into_iter()
                .map(|h| h as i64)
                .collect();
            let bytes: Vec<u8> = sig.iter().flat_map(|v| v.to_le_bytes()).collect();
            db::minhash_upsert(
                &ctx.state.db,
                &doc.id,
                &ctx.minhash_config_hash,
                &fingerprint,
                &bytes,
                &bands,
            )
            .await?;
            stats.signatures_written += 1;
        }
    }

    Ok(reasons)
}

/// Content-change fingerprint: the stored hash when Onyx computed one, else
/// the cheap composite that moves whenever the document is re-crawled.
fn fingerprint_of(doc: &ScanDocRow) -> String {
    match &doc.content_hash {
        Some(hash) => hash.clone(),
        None => format!("cc{:?}-{}", doc.chunk_count, doc.updated_at.timestamp()),
    }
}

// ---------------------------------------------------------------------------
// Near-duplicate pair verification
// ---------------------------------------------------------------------------

async fn near_pairs_for_bucket(
    ctx: &ScanCtx<'_>,
    members: &[(String, Vec<u8>)],
    seen_pairs: &mut HashSet<(String, String)>,
    stats: &mut ScanStats,
) -> Result<(), crate::error::AppError> {
    let Some(engine) = &ctx.engine else {
        return Ok(());
    };
    if members.len() < 2 {
        return Ok(());
    }

    let sigs: Vec<(String, Vec<u64>)> = members
        .iter()
        .map(|(id, bytes)| {
            let sig: Vec<u64> = bytes
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                .collect();
            (id.clone(), sig)
        })
        .collect();

    // Verify each new pair; collect the ones above the report floor.
    let mut hits: Vec<(String, String, f64)> = Vec::new();
    for i in 0..sigs.len() {
        for j in (i + 1)..sigs.len() {
            let key = (sigs[i].0.clone(), sigs[j].0.clone());
            if !seen_pairs.insert(key) {
                continue;
            }
            stats.near_pairs_verified += 1;
            let sim = engine.jaccard_similarity(&sigs[i].1, &sigs[j].1);
            if sim >= ctx.config.dedup.report_only_low {
                hits.push((sigs[i].0.clone(), sigs[j].0.clone(), sim));
            }
        }
    }
    if hits.is_empty() {
        return Ok(());
    }

    // Resolve document rows once per bucket: keeper metadata (any scope) and
    // scope membership (which documents may be flagged).
    let mut ids: Vec<String> = hits
        .iter()
        .flat_map(|(a, b, _)| [a.clone(), b.clone()])
        .collect();
    ids.sort();
    ids.dedup();
    let rows = db::scan_documents_by_ids(&ctx.state.db, None, &ids).await?;
    let in_scope: HashSet<String> =
        db::scan_documents_by_ids(&ctx.state.db, Some(&ctx.scan.scope), &ids)
            .await?
            .into_iter()
            .map(|d| d.id)
            .collect();
    let by_id: HashMap<&str, &ScanDocRow> = rows.iter().map(|d| (d.id.as_str(), d)).collect();

    // Every verified pair is stored, including ones below the acting
    // threshold. That is what makes the threshold a review-time decision: the
    // dial can be lowered later without recomputing a single signature.
    let pairs: Vec<profile_db::DupPair> = hits
        .iter()
        .filter_map(|(a, b, sim)| {
            let (doc_a, doc_b) = (by_id.get(a.as_str())?, by_id.get(b.as_str())?);
            Some(profile_db::DupPair {
                a: a.clone(),
                b: b.clone(),
                method: "minhash".into(),
                estimated: Some(*sim as f32),
                verified: None,
                cosine: None,
                same_connector: Some(doc_a.connector_id == doc_b.connector_id),
            })
        })
        .collect();
    match profile_db::upsert_pairs(&ctx.state.db, &pairs).await {
        Ok(n) => stats.pairs_stored += n as i64,
        Err(err) => tracing::warn!(error = %err, "storing duplicate pairs failed"),
    }

    // Both sides record the similarity: policy thresholds read the profile,
    // and which side ends up flagged is a keeper decision made below.
    let similarities: Vec<(String, f32, String)> = hits
        .iter()
        .flat_map(|(a, b, sim)| {
            [
                (a.clone(), *sim as f32, b.clone()),
                (b.clone(), *sim as f32, a.clone()),
            ]
        })
        .collect();
    if let Err(err) = profile_db::set_max_similarity(&ctx.state.db, "minhash", &similarities).await
    {
        tracing::warn!(error = %err, "recording maximum similarity failed");
    }

    for (a, b, sim) in hits {
        let (Some(doc_a), Some(doc_b)) = (by_id.get(a.as_str()), by_id.get(b.as_str())) else {
            continue; // one side vanished between phases
        };
        let group = [*doc_a, *doc_b];
        let keeper = select_keeper(&group, ctx.config.dedup.prefer_keep);
        let flagged = if keeper.id == doc_a.id {
            *doc_b
        } else {
            *doc_a
        };
        if !in_scope.contains(&flagged.id) {
            continue;
        }
        let report_only = sim < ctx.config.dedup.similarity_threshold;
        let hit = hit_from(
            flagged,
            vec![PruneReason {
                detector: "duplicate".into(),
                code: "near_duplicate_of".into(),
                detail: format!(
                    "{:.0}% similar to {} (threshold {:.0}%{}; keeper by {})",
                    sim * 100.0,
                    keeper.id,
                    ctx.config.dedup.similarity_threshold * 100.0,
                    if report_only {
                        "; below it — report only"
                    } else {
                        ""
                    },
                    policy_name(ctx.config.dedup.prefer_keep),
                ),
                confidence: sim as f32,
                evidence: json!({
                    "kept": keeper.id,
                    "similarity": sim,
                    "threshold": ctx.config.dedup.similarity_threshold,
                    "report_only": report_only,
                    "policy": policy_name(ctx.config.dedup.prefer_keep),
                }),
            }],
        );
        stats.near_hits += 1;
        let outcome = db::upsert_candidate(&ctx.state.db, Some(ctx.scan.id), &hit).await?;
        stats.record(outcome);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Row-level detectors
// ---------------------------------------------------------------------------

fn recrawl_risk_of(doc: &ScanDocRow) -> bool {
    doc.cc_pair_status
        .as_deref()
        .map(|s| RECRAWLING_STATES.contains(&s))
        .unwrap_or(false)
}

fn hit_from(doc: &ScanDocRow, reasons: Vec<PruneReason>) -> DetectorHit {
    DetectorHit {
        document_id: doc.id.clone(),
        reasons,
        connector_id: doc.connector_id,
        cc_pair_id: doc.cc_pair_id,
        chunk_count: doc.chunk_count,
        recrawl_risk: recrawl_risk_of(doc),
    }
}

/// The stub detector: `chunk_count == 0` (never NULL — "not counted yet" is
/// not "empty"), age-gated so freshly crawled pages that simply have not been
/// chunked yet never appear.
fn stub_reason(doc: &ScanDocRow, config: &PruneConfig) -> Option<PruneReason> {
    if doc.chunk_count != Some(0) {
        return None;
    }
    let age_days = (Utc::now() - doc.updated_at).num_days();
    if age_days < config.thin.min_age_days {
        return None;
    }
    Some(PruneReason {
        detector: "thin".into(),
        code: "chunkless_stub".into(),
        detail: format!(
            "0 chunks, {age_days} days after its last crawl touch (gate: {} days)",
            config.thin.min_age_days
        ),
        confidence: config.thin.stub_confidence,
        evidence: json!({
            "chunk_count": 0,
            "age_days": age_days,
            "min_age_days": config.thin.min_age_days,
        }),
    })
}

/// Stale: old content on a pair that is still crawling (the page stopped
/// changing or vanished upstream). Policy, not junk — report-only shape.
fn stale_reason(doc: &ScanDocRow, config: &PruneConfig) -> Option<PruneReason> {
    if !recrawl_risk_of(doc) {
        return None; // a paused pair's documents are "old" by definition
    }
    let age_days = (Utc::now() - doc.updated_at).num_days();
    if age_days < config.stale.older_than_days {
        return None;
    }
    Some(PruneReason {
        detector: "stale".into(),
        code: "stale_content".into(),
        detail: format!(
            "not updated for {age_days} days on an actively crawling connector \
             (threshold: {} days)",
            config.stale.older_than_days
        ),
        confidence: config.stale.confidence,
        evidence: json!({
            "age_days": age_days,
            "older_than_days": config.stale.older_than_days,
        }),
    })
}

fn url_rule_reason(doc: &ScanDocRow, rule: &CompiledRule) -> PruneReason {
    let matched = rule
        .regex
        .find(&doc.id)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    PruneReason {
        detector: "url_rule".into(),
        // The rule name, so two different rules never fold into one reason.
        code: rule.name.clone(),
        detail: format!(
            "URL matches rule '{}' (pattern: {})",
            rule.name, rule.pattern
        ),
        confidence: rule.confidence,
        evidence: json!({
            "rule": rule.name,
            "pattern": rule.pattern,
            "matched": matched,
        }),
    }
}

fn tag_rule_reason(rule: &CompiledRule, key: &str, value: &str) -> PruneReason {
    PruneReason {
        detector: "tag_rule".into(),
        code: rule.name.clone(),
        detail: format!("tag {key}={value} matches rule '{}'", rule.name),
        confidence: rule.confidence,
        evidence: json!({
            "rule": rule.name,
            "pattern": rule.pattern,
            "tag_key": key,
            "tag_value": value,
        }),
    }
}

fn exact_dup_hit(
    member: &ScanDocRow,
    keeper: &ScanDocRow,
    content_hash: &str,
    group_size: usize,
    policy: PreferKeepPolicy,
) -> DetectorHit {
    hit_from(
        member,
        vec![PruneReason {
            detector: "duplicate".into(),
            code: "exact_duplicate_of".into(),
            detail: format!(
                "identical content hash as {} (group of {group_size}; keeper chosen by {})",
                keeper.id,
                policy_name(policy)
            ),
            confidence: 1.0,
            evidence: json!({
                "kept": keeper.id,
                "similarity": 1.0,
                "content_hash": content_hash,
                "group_size": group_size,
                "policy": policy_name(policy),
            }),
        }],
    )
}

fn policy_name(policy: PreferKeepPolicy) -> &'static str {
    match policy {
        PreferKeepPolicy::ShortestUrl => "shortest_url",
        PreferKeepPolicy::LongestContent => "longest_content",
        PreferKeepPolicy::NewestUpdated => "newest_updated",
        PreferKeepPolicy::MostChunks => "most_chunks",
    }
}

/// Whether a duplicate group draws its members from more than one connector.
///
/// Recorded on every member's profile so policy can hold cross-connector
/// copies to review without a self-join at read time. A member whose connector
/// is unknown counts as its own source: "we cannot tell" must not read as
/// "same connector", because the whole point of the flag is to be cautious.
fn spans_connectors(group: &[&ScanDocRow]) -> bool {
    let mut seen: Option<Option<i32>> = None;
    for member in group {
        match seen {
            None => seen = Some(member.connector_id),
            Some(first) if first == member.connector_id && member.connector_id.is_some() => {}
            Some(_) => return true,
        }
    }
    false
}

/// Pick the group member that survives. Content is not fetched for exact
/// groups (the hash already proves identity), so `longest_content` is served
/// by chunk count — same ordering, no fetch.
fn select_keeper<'a>(group: &[&'a ScanDocRow], policy: PreferKeepPolicy) -> &'a ScanDocRow {
    let url_len = |d: &ScanDocRow| d.link.as_deref().map(str::len).unwrap_or(d.id.len());
    group
        .iter()
        .copied()
        .min_by(|a, b| {
            let ordering = match policy {
                PreferKeepPolicy::ShortestUrl => url_len(a).cmp(&url_len(b)),
                PreferKeepPolicy::LongestContent | PreferKeepPolicy::MostChunks => b
                    .chunk_count
                    .unwrap_or(-1)
                    .cmp(&a.chunk_count.unwrap_or(-1)),
                PreferKeepPolicy::NewestUpdated => b.updated_at.cmp(&a.updated_at),
            };
            ordering.then_with(|| a.id.cmp(&b.id))
        })
        .expect("groups have at least two members")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn doc(id: &str, chunk_count: Option<i32>, age_days: i64) -> ScanDocRow {
        ScanDocRow {
            id: id.to_string(),
            semantic_id: id.to_string(),
            link: Some(id.to_string()),
            chunk_count,
            content_hash: None,
            updated_at: Utc::now() - Duration::days(age_days),
            hidden: false,
            connector_id: Some(1),
            connector_name: Some("test-connector".into()),
            cc_pair_id: Some(1),
            cc_pair_status: Some("PAUSED".into()),
        }
    }

    #[test]
    fn null_chunk_count_is_never_a_stub() {
        let config = PruneConfig::default();
        assert!(stub_reason(&doc("https://a/x", None, 100), &config).is_none());
        assert!(stub_reason(&doc("https://a/x", Some(0), 100), &config).is_some());
        assert!(stub_reason(&doc("https://a/x", Some(3), 100), &config).is_none());
    }

    #[test]
    fn fresh_stubs_are_age_gated() {
        let config = PruneConfig::default(); // min_age_days 7
        assert!(stub_reason(&doc("https://a/x", Some(0), 2), &config).is_none());
        assert!(stub_reason(&doc("https://a/x", Some(0), 8), &config).is_some());
    }

    #[test]
    fn stub_reason_carries_typed_evidence() {
        let config = PruneConfig::default();
        let reason = stub_reason(&doc("https://a/x", Some(0), 30), &config).unwrap();
        assert_eq!(reason.detector, "thin");
        assert_eq!(reason.code, "chunkless_stub");
        assert_eq!(reason.confidence, 0.9);
        assert_eq!(reason.evidence["chunk_count"], 0);
        assert_eq!(reason.evidence["min_age_days"], 7);
    }

    #[test]
    fn stale_only_fires_on_actively_crawling_pairs() {
        let mut config = PruneConfig::default();
        config.stale.older_than_days = 30;
        let paused = doc("https://a/x", Some(3), 100);
        assert!(
            stale_reason(&paused, &config).is_none(),
            "a paused pair's age is not staleness"
        );
        let mut active = doc("https://a/x", Some(3), 100);
        active.cc_pair_status = Some("ACTIVE".into());
        assert!(stale_reason(&active, &config).is_some());
        let mut fresh = doc("https://a/x", Some(3), 10);
        fresh.cc_pair_status = Some("ACTIVE".into());
        assert!(stale_reason(&fresh, &config).is_none());
    }

    #[test]
    fn url_rule_reasons_use_the_rule_name_as_code() {
        let rule = CompiledRule {
            name: "calendar-pages".into(),
            pattern: r"/calendar/\d{4}".into(),
            regex: regex::Regex::new(r"/calendar/\d{4}").unwrap(),
            confidence: 0.8,
        };
        let d = doc("https://a/calendar/2024/05", Some(2), 10);
        let reason = url_rule_reason(&d, &rule);
        assert_eq!(reason.detector, "url_rule");
        assert_eq!(
            reason.code, "calendar-pages",
            "distinct rules must not fold"
        );
        assert_eq!(reason.evidence["matched"], "/calendar/2024");
    }

    #[test]
    fn recrawl_risk_follows_cc_pair_status() {
        let mut d = doc("https://a/x", Some(0), 30);
        assert!(!recrawl_risk_of(&d));
        d.cc_pair_status = Some("ACTIVE".into());
        assert!(recrawl_risk_of(&d));
        d.cc_pair_status = Some("INITIAL_INDEXING".into());
        assert!(recrawl_risk_of(&d));
        d.cc_pair_status = None;
        assert!(!recrawl_risk_of(&d));
    }

    #[test]
    fn keeper_selection_is_deterministic_per_policy() {
        let short = doc("https://a/x", Some(5), 10);
        let long = doc("https://a/x/very/deep/duplicate", Some(9), 2);
        let group = vec![&short, &long];

        assert_eq!(
            select_keeper(&group, PreferKeepPolicy::ShortestUrl).id,
            "https://a/x"
        );
        assert_eq!(
            select_keeper(&group, PreferKeepPolicy::MostChunks).id,
            "https://a/x/very/deep/duplicate"
        );
        assert_eq!(
            select_keeper(&group, PreferKeepPolicy::NewestUpdated).id,
            "https://a/x/very/deep/duplicate"
        );

        let a = doc("https://a/a", Some(5), 10);
        let b = doc("https://a/b", Some(5), 10);
        assert_eq!(
            select_keeper(&[&a, &b], PreferKeepPolicy::MostChunks).id,
            select_keeper(&[&b, &a], PreferKeepPolicy::MostChunks).id,
        );
    }

    #[test]
    fn exact_dup_reason_names_the_keeper_and_similarity() {
        let keeper = doc("https://a/x", Some(5), 10);
        let member = doc("https://a/x?utm_source=feed", Some(5), 10);
        let hit = exact_dup_hit(&member, &keeper, "abc123", 2, PreferKeepPolicy::ShortestUrl);
        assert_eq!(hit.reasons[0].confidence, 1.0);
        assert_eq!(hit.reasons[0].evidence["kept"], "https://a/x");
        assert_eq!(hit.reasons[0].evidence["similarity"], 1.0);
        assert_eq!(hit.reasons[0].evidence["policy"], "shortest_url");
    }

    #[test]
    fn band_cursors_round_trip() {
        assert_eq!(parse_band_cursor("3:12345"), Some((3, 12345)));
        assert_eq!(parse_band_cursor("3:-9871234"), Some((3, -9871234)));
        assert_eq!(parse_band_cursor("garbage"), None);
    }

    #[test]
    fn fingerprints_prefer_the_stored_content_hash() {
        let mut d = doc("https://a/x", Some(5), 10);
        d.content_hash = Some("stored-hash".into());
        assert_eq!(fingerprint_of(&d), "stored-hash");
        d.content_hash = None;
        assert!(fingerprint_of(&d).starts_with("cc"));
    }
}

#[cfg(test)]
mod version_skew_tests {
    use super::*;
    use crate::error::AppError;

    /// A scan queued by a newer build must not be marked failed by an older
    /// one. Both instances poll the same table during a rolling upgrade, and
    /// the old build burning the retry budget would blame the scan for a
    /// deployment state. Observed live against gamma.
    #[test]
    fn a_config_snapshot_from_a_newer_build_is_recognised_and_not_a_normal_failure() {
        let too_new = AppError::BadRequest(
            "scan config snapshot: unknown field `quality`, expected one of `version`, `dedup`"
                .into(),
        );
        assert!(is_config_too_new(&too_new));

        // Ordinary failures must still count toward the retry budget.
        for ordinary in [
            AppError::BadRequest("scan config snapshot: invalid type: string".into()),
            AppError::UpstreamSearch("connection reset by peer".into()),
            AppError::Database("deadlock detected".into()),
        ] {
            assert!(
                !is_config_too_new(&ordinary),
                "{ordinary:?} is a real failure, not version skew"
            );
        }
    }
}
