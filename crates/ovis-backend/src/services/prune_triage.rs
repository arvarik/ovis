//! Review at scale: the funnel, the threshold dial, policy simulation, and
//! cluster review.
//!
//! The v1 review surface was a list of candidates. On the reference deployment
//! that list is 207,230 rows long and five actions were taken against it in a
//! week — the shape of the UI, not the quality of the detection, was the
//! bottleneck. Nobody reviews six figures of anything one row at a time.
//!
//! So the unit of review here is the *aggregate*: bundles grouped by reason
//! and source, thresholds moved against a live distribution, duplicate
//! clusters approved whole, and — for the large homogeneous groups that make
//! up most of the backlog — a statistical sample standing in for the group.
//! Item-level review remains, but as the residual case rather than the
//! workflow.
//!
//! Nothing here deletes. Simulation is read-only by construction, committing a
//! policy creates candidates in the existing lifecycle, and everything past
//! that point is the v1 machinery unchanged.

use ovis_core::api_types::PruneReason;
use ovis_core::db::documents;
use ovis_core::db::profile::{self as profile_db, Band, Policy};
use ovis_core::db::prune as db;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::services::prune::{actor, guard};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// The funnel
// ---------------------------------------------------------------------------

/// One reviewable group: what it is, how big, and what reviewing it buys back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    /// Stable key the UI turns into a pre-filtered view.
    pub key: String,
    pub title: String,
    /// One honest sentence about what this group is and how it was found.
    pub description: String,
    pub detector: Option<String>,
    pub documents: i64,
    /// Chunks these documents hold — the index weight, which is what deleting
    /// them actually reclaims.
    pub chunks: i64,
    /// Mean confidence across the group, so an operator can tell a group of
    /// certainties from a group of guesses at a glance.
    pub mean_confidence: f64,
    /// Documents in the group sitting on a still-crawling connector.
    pub recrawl_risk: i64,
    /// A generated title and summary, when one exists. Filled in by the route
    /// layer, so this module stays unaware of the LLM subsystem and a
    /// deployment with no model configured sees exactly today's behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narration: Option<crate::services::narrate::NarrationView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewResponse {
    pub candidates_open: i64,
    pub documents_total: i64,
    pub profiled: i64,
    pub pairs: i64,
    pub bundles: Vec<Bundle>,
    pub by_connector: Vec<ConnectorBundle>,
    pub trash: ovis_core::db::trash::TrashCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorBundle {
    pub connector_id: Option<i32>,
    pub connector_name: Option<String>,
    pub documents: i64,
    pub chunks: i64,
    pub mean_confidence: f64,
}

/// Human-readable descriptions of each reason code. The review UI shows these
/// instead of the code, because "chunkless_stub" is not a sentence anyone can
/// act on.
fn describe(code: &str) -> (&'static str, &'static str) {
    match code {
        "exact_duplicate_of" => (
            "Identical copies",
            "Byte-for-byte the same extracted content as another document. The keeper is \
             chosen by the configured policy; every other copy is redundant.",
        ),
        "near_duplicate_of" => (
            "Near-identical copies",
            "Verified overlapping text with another document — mirrors, reposts and \
             http/https pairs. The measured similarity is on each candidate.",
        ),
        "url_variant_of" => (
            "Same page, different URL",
            "The canonical URL matches another document once tracking parameters, scheme, \
             www and trailing slashes are folded. Content hashes differ, so exact-duplicate \
             detection cannot see these.",
        ),
        "asset_url" => (
            "Files indexed as pages",
            "Image, media and archive URLs whose extracted text is the filename and \
             dimensions rather than content. Searching never wants these.",
        ),
        "chunkless_stub" => (
            "Empty documents",
            "Indexed with zero chunks well after their last crawl — the page produced no \
             extractable text at all.",
        ),
        "thin_content" => (
            "Very short documents",
            "Fewer words than the configured floor. Often placeholder or error pages.",
        ),
        "low_quality_text" => (
            "Low-quality text",
            "Failed several published text-quality checks across different categories — \
             navigation chrome, listings, repeated boilerplate. Review before staging: \
             reference pages full of code and tables can look the same to these checks.",
        ),
        "lang_not_allowed" => (
            "Unexpected language",
            "Detected a language outside the configured allow-list.",
        ),
        "stale_content" => (
            "Stale documents",
            "Old content on a connector that is still crawling — the page stopped changing \
             or vanished upstream.",
        ),
        "recrawled_after_prune" => (
            "Returned after pruning",
            "Previously pruned with remember, and the crawler brought it back. Re-staged \
             automatically, never deleted without the full grace period.",
        ),
        // `commit` writes these two, so they are not user-authored rules and
        // must not fall through to the rule wording below — after a policy
        // commit they are usually the largest group on the screen.
        "policy_auto" => (
            "Matched the bulk band",
            "Crossed the committed policy's stronger thresholds. Each candidate lists the \
             signals that put it here; staging is still a separate, confirmed step.",
        ),
        "policy_review" => (
            "Matched the review band",
            "Crossed the committed policy's review thresholds but not its bulk ones — a human \
             decision by construction. Each candidate lists the signals that put it here.",
        ),
        _ => (
            "Other",
            "Flagged by a user-authored rule; the rule name is on each candidate.",
        ),
    }
}

pub async fn overview(state: &AppState) -> Result<OverviewResponse, AppError> {
    guard(state)?;

    // One aggregate over the candidate table, grouped by reason code. The GIN
    // index on `reasons` does not serve this, but at 207k rows the sequential
    // aggregate is well under a second and it runs once per page load.
    //
    // The lateral is reduced to one row per (document, code, detector) before
    // anything is summed. Unnesting `reasons` multiplies a candidate by its
    // reason count, so a document whose reasons repeat a code — two URL rules
    // sharing a name, a re-scan appending an equivalent reason — would be
    // counted once under `documents` and twice under `chunks` and
    // `recrawl_risk`, and a card would claim more weight than the group holds.
    let rows = sqlx::query(
        "SELECT code, detector, \
                count(*) AS documents, \
                COALESCE(sum(chunks), 0)::bigint AS chunks, \
                avg(confidence)::float8 AS mean_confidence, \
                count(*) FILTER (WHERE recrawl_risk) AS recrawl_risk \
         FROM ( \
             SELECT DISTINCT ON (pc.document_id, el->>'code', el->>'detector') \
                    el->>'code' AS code, \
                    el->>'detector' AS detector, \
                    COALESCE(d.chunk_count, pc.chunk_count) AS chunks, \
                    pc.confidence, pc.recrawl_risk \
             FROM ovis.prune_candidate pc \
             LEFT JOIN public.document d ON d.id = pc.document_id \
             CROSS JOIN LATERAL jsonb_array_elements(pc.reasons) el \
             WHERE pc.state = 'candidate' \
         ) reasons \
         GROUP BY code, detector ORDER BY documents DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    let bundles: Vec<Bundle> = rows
        .iter()
        .map(|r| {
            let code: String = r.try_get_code();
            let (title, description) = describe(&code);
            Bundle {
                key: code.clone(),
                title: title.to_string(),
                description: description.to_string(),
                detector: r.try_get_detector(),
                documents: r.try_get_i64("documents"),
                chunks: r.try_get_i64("chunks"),
                mean_confidence: r.try_get_f64("mean_confidence"),
                recrawl_risk: r.try_get_i64("recrawl_risk"),
                narration: None,
            }
        })
        .collect();

    let connector_rows = sqlx::query(
        "SELECT pc.connector_id, c.name AS connector_name, \
                count(*) AS documents, \
                COALESCE(sum(COALESCE(d.chunk_count, pc.chunk_count)), 0)::bigint AS chunks, \
                avg(pc.confidence)::float8 AS mean_confidence \
         FROM ovis.prune_candidate pc \
         LEFT JOIN public.document d ON d.id = pc.document_id \
         LEFT JOIN public.connector c ON c.id = pc.connector_id \
         WHERE pc.state = 'candidate' \
         GROUP BY 1, 2 ORDER BY documents DESC LIMIT 50",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    let counts = db::state_counts(&state.db).await?;
    let documents_total: i64 = sqlx::query_scalar("SELECT count(*) FROM public.document")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    Ok(OverviewResponse {
        candidates_open: counts.candidates,
        documents_total,
        profiled: profile_db::profile_count(&state.db).await.unwrap_or(0),
        pairs: profile_db::pair_count(&state.db).await.unwrap_or(0),
        bundles,
        by_connector: connector_rows
            .iter()
            .map(|r| ConnectorBundle {
                connector_id: r.try_get_i32("connector_id"),
                connector_name: r.try_get_string("connector_name"),
                documents: r.try_get_i64("documents"),
                chunks: r.try_get_i64("chunks"),
                mean_confidence: r.try_get_f64("mean_confidence"),
            })
            .collect(),
        trash: ovis_core::db::trash::counts(&state.db)
            .await
            .unwrap_or_default(),
    })
}

/// Small row helpers: these aggregates mix nullable and non-nullable columns
/// and the explicit `try_get` chain at every call site drowns the query.
trait RowExt {
    fn try_get_code(&self) -> String;
    fn try_get_detector(&self) -> Option<String>;
    fn try_get_i64(&self, name: &str) -> i64;
    fn try_get_i32(&self, name: &str) -> Option<i32>;
    fn try_get_f64(&self, name: &str) -> f64;
    fn try_get_string(&self, name: &str) -> Option<String>;
}

impl RowExt for sqlx::postgres::PgRow {
    fn try_get_code(&self) -> String {
        use sqlx::Row;
        self.try_get::<Option<String>, _>("code")
            .ok()
            .flatten()
            .unwrap_or_else(|| "unknown".into())
    }
    fn try_get_detector(&self) -> Option<String> {
        use sqlx::Row;
        self.try_get::<Option<String>, _>("detector").ok().flatten()
    }
    fn try_get_i64(&self, name: &str) -> i64 {
        use sqlx::Row;
        self.try_get::<Option<i64>, _>(name)
            .ok()
            .flatten()
            .unwrap_or(0)
    }
    fn try_get_i32(&self, name: &str) -> Option<i32> {
        use sqlx::Row;
        self.try_get::<Option<i32>, _>(name).ok().flatten()
    }
    fn try_get_f64(&self, name: &str) -> f64 {
        use sqlx::Row;
        self.try_get::<Option<f64>, _>(name)
            .ok()
            .flatten()
            .unwrap_or(0.0)
    }
    fn try_get_string(&self, name: &str) -> Option<String> {
        use sqlx::Row;
        self.try_get::<Option<String>, _>(name).ok().flatten()
    }
}

// ---------------------------------------------------------------------------
// Policy: simulate and commit
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulateRequest {
    /// A preset name, or `policy` for an explicit body. Exactly one.
    pub tier: Option<String>,
    pub policy: Option<Policy>,
    /// Draw this many random documents from each band as a boundary check.
    #[serde(default)]
    pub sample: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimulateResponse {
    pub tier: Option<String>,
    pub policy: Policy,
    pub policy_hash: String,
    #[serde(flatten)]
    pub result: profile_db::SimulationResult,
    /// Random members of each band, so a threshold is checked against real
    /// documents rather than accepted on a count alone.
    pub auto_sample: Vec<SampleDoc>,
    pub review_sample: Vec<SampleDoc>,
    /// What this policy cannot see yet, stated plainly rather than left to be
    /// inferred from a zero.
    pub caveats: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SampleDoc {
    pub document_id: String,
    pub semantic_id: Option<String>,
    pub chunk_count: Option<i32>,
    pub signals: Vec<String>,
}

pub fn policy_from_request(
    tier: Option<&str>,
    policy: Option<&Policy>,
) -> Result<(Option<String>, Policy), AppError> {
    let resolved = match (tier, policy) {
        (Some(tier), None) => {
            let policy = Policy::by_tier(tier).ok_or_else(|| {
                AppError::BadRequest(format!(
                    "unknown tier '{tier}'; expected conservative, standard or aggressive"
                ))
            })?;
            (Some(tier.to_string()), policy)
        }
        (None, Some(policy)) => (None, policy.clone()),
        (Some(_), Some(_)) => {
            return Err(AppError::BadRequest(
                "pass either tier or policy, not both".into(),
            ))
        }
        (None, None) => (Some("standard".into()), Policy::standard()),
    };
    resolved.1.validate().map_err(AppError::BadRequest)?;
    Ok(resolved)
}

pub fn policy_hash(policy: &Policy) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_string(policy).unwrap_or_default().as_bytes());
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Evaluate a policy against the stored profiles. **Mutates nothing.**
pub async fn simulate(
    state: &AppState,
    request: SimulateRequest,
) -> Result<SimulateResponse, AppError> {
    guard(state)?;
    let (tier, policy) = policy_from_request(request.tier.as_deref(), request.policy.as_ref())?;
    let result = profile_db::simulate(&state.db, &policy).await?;

    let sample_size = request.sample.unwrap_or(0).clamp(0, 100);
    let auto_sample = if sample_size > 0 {
        hydrate_sample(
            state,
            &profile_db::sample_band(&state.db, &policy, Band::Auto, sample_size).await?,
            &policy,
        )
        .await?
    } else {
        Vec::new()
    };
    let review_sample = if sample_size > 0 {
        hydrate_sample(
            state,
            &profile_db::sample_band(&state.db, &policy, Band::Review, sample_size).await?,
            &policy,
        )
        .await?
    } else {
        Vec::new()
    };

    Ok(SimulateResponse {
        tier,
        policy_hash: policy_hash(&policy),
        caveats: caveats_for(state, &policy, &result).await,
        policy,
        result,
        auto_sample,
        review_sample,
    })
}

/// Group digits for display. These caveats are prose shown to a person, and
/// "2403 of 1738163" is measurably harder to read than "2,403 of 1,738,163".
fn thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Say what the numbers do *not* cover. A simulation that silently reports
/// zero semantic duplicates because nothing measured cosine yet is worse than
/// one that says so.
async fn caveats_for(
    state: &AppState,
    policy: &Policy,
    result: &profile_db::SimulationResult,
) -> Vec<String> {
    let mut caveats = Vec::new();
    let documents_total: i64 = sqlx::query_scalar("SELECT count(*) FROM public.document")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    if result.profiled < documents_total {
        caveats.push(format!(
            "{} of {} documents have been measured; the rest are invisible to this simulation \
             until a scan covers them.",
            thousands(result.profiled),
            thousands(documents_total)
        ));
    }
    let measured_cosine: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ovis.doc_profile WHERE max_cosine IS NOT NULL")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    if policy.semantic.review.is_some() && measured_cosine == 0 {
        caveats.push(
            "Semantic thresholds are set but no document has an embedding similarity yet — run \
             a semantic scan, or these thresholds contribute nothing."
                .into(),
        );
    }
    let measured_jaccard: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ovis.doc_profile WHERE max_jaccard IS NOT NULL")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    if policy.near_duplicate.review.is_some() && measured_jaccard == 0 {
        caveats.push(
            "Near-duplicate thresholds are set but no signatures have been compared yet — run a \
             near_duplicate scan first."
                .into(),
        );
    }
    if policy.quality.auto_min_failures.is_some() {
        caveats.push(
            "This policy lets text-quality checks stage documents without review. Those checks \
             identify unusual text, which includes reference pages full of code and tables."
                .into(),
        );
    }
    caveats
}

async fn hydrate_sample(
    state: &AppState,
    ids: &[String],
    policy: &Policy,
) -> Result<Vec<SampleDoc>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let docs = documents::documents_by_ids(&state.db, ids, None).await?;
    let by_document = profile_db::signals_for_documents(&state.db, policy, ids)
        .await
        .unwrap_or_default();
    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        let signals = by_document
            .get(&doc.id)
            .map(|hits| {
                hits.iter()
                    .map(|(signal, band)| format!("{signal} ({band})"))
                    .collect()
            })
            .unwrap_or_default();
        out.push(SampleDoc {
            document_id: doc.id,
            semantic_id: Some(doc.semantic_id),
            chunk_count: doc.chunk_count,
            signals,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    pub tier: Option<String>,
    pub policy: Option<Policy>,
    /// Which band to turn into candidates.
    pub band: String,
    /// The count the caller believes it is acting on, as everywhere else.
    pub confirm_count: i64,
    /// Save the policy under this name and make it the active one.
    pub save_as: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitResponse {
    pub band: String,
    pub policy_hash: String,
    pub created: i64,
    pub skipped: i64,
    pub saved_as: Option<String>,
}

/// Turn a band into candidates.
///
/// Creates review rows only — staging still goes through the existing
/// confirm-count bulk endpoint, and deletion still goes through the grace
/// period and the reaper. Committing a policy is the *cheapest* thing in the
/// lifecycle, not a shortcut past it.
pub async fn commit(state: &AppState, request: CommitRequest) -> Result<CommitResponse, AppError> {
    guard(state)?;
    let (tier, policy) = policy_from_request(request.tier.as_deref(), request.policy.as_ref())?;
    let band = match request.band.as_str() {
        "auto" => Band::Auto,
        "review" => Band::Review,
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown band '{other}'; expected auto or review"
            )))
        }
    };

    let result = profile_db::simulate(&state.db, &policy).await?;
    let expected = match band {
        Band::Auto => result.auto,
        Band::Review => result.review,
        Band::None => 0,
    };
    if expected != request.confirm_count {
        return Err(AppError::Conflict(format!(
            "the {} band currently holds {expected} documents, not the confirmed {}; nothing was \
             created. Re-simulate and resend with confirm_count={expected}",
            request.band, request.confirm_count
        )));
    }

    let hash = policy_hash(&policy);
    let who = actor(state);
    let mut created = 0i64;
    let mut skipped = 0i64;
    let mut cursor: Option<String> = None;

    loop {
        let ids = profile_db::documents_in_band(&state.db, &policy, band, cursor.as_deref(), 1000)
            .await?;
        if ids.is_empty() {
            break;
        }
        let rows = db::scan_documents_by_ids(&state.db, None, &ids).await?;
        // One query for the whole page rather than one per signal per
        // document; a large band is otherwise millions of round trips.
        let by_document = profile_db::signals_for_documents(&state.db, &policy, &ids).await?;
        let mut hits = Vec::with_capacity(rows.len());
        for doc in &rows {
            let Some(signals) = by_document.get(&doc.id) else {
                skipped += 1;
                continue;
            };
            let detail = signals
                .iter()
                .map(|(signal, band)| format!("{signal} ({band})"))
                .collect::<Vec<_>>()
                .join(", ");
            hits.push(db::DetectorHit {
                document_id: doc.id.clone(),
                reasons: vec![PruneReason {
                    detector: "policy".into(),
                    code: format!("policy_{}", request.band),
                    detail: format!("matched the {} policy on: {detail}", request.band),
                    confidence: match band {
                        Band::Auto => 0.95,
                        _ => 0.7,
                    },
                    evidence: json!({
                        "band": request.band,
                        "tier": tier,
                        "policy_hash": hash,
                        "signals": signals.iter().map(|(s, _)| s).collect::<Vec<_>>(),
                    }),
                }],
                connector_id: doc.connector_id,
                cc_pair_id: doc.cc_pair_id,
                chunk_count: doc.chunk_count,
                recrawl_risk: doc
                    .cc_pair_status
                    .as_deref()
                    .map(|s| ["ACTIVE", "INITIAL_INDEXING"].contains(&s))
                    .unwrap_or(false),
            });
        }
        for outcome in db::upsert_candidates(&state.db, None, &hits).await? {
            match outcome {
                db::UpsertOutcome::Inserted => created += 1,
                _ => skipped += 1,
            }
        }
        cursor = ids.last().cloned();
        tokio::task::yield_now().await;
    }

    let saved_as = match &request.save_as {
        Some(name) => {
            let body = serde_json::to_value(&policy)
                .map_err(|e| AppError::BadRequest(format!("unserialisable policy: {e}")))?;
            profile_db::save_policy(
                &state.db,
                name,
                tier.as_deref().unwrap_or("custom"),
                &body,
                &hash,
            )
            .await?;
            profile_db::activate_policy(&state.db, name).await?;
            Some(name.clone())
        }
        None => None,
    };

    db::audit(
        &state.db,
        who,
        "policy_committed",
        None,
        None,
        None,
        Some(json!({
            "band": request.band,
            "tier": tier,
            "policy_hash": hash,
            "created": created,
            "skipped": skipped,
            "saved_as": saved_as,
        })),
    )
    .await;

    Ok(CommitResponse {
        band: request.band,
        policy_hash: hash,
        created,
        skipped,
        saved_as,
    })
}

// ---------------------------------------------------------------------------
// Clusters
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ClusterMember {
    pub document_id: String,
    pub semantic_id: Option<String>,
    pub link: Option<String>,
    pub chunk_count: Option<i32>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_keeper: bool,
    pub candidate_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cluster {
    pub key: String,
    pub method: String,
    pub size: i64,
    pub members: Vec<ClusterMember>,
    /// Which rule chose the keeper, in words.
    pub keeper_reason: String,
    /// A generated title and summary, when one exists. See [`Bundle`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narration: Option<crate::services::narrate::NarrationView>,
}

/// Duplicate clusters, keeper first.
///
/// One cluster per screen is the review unit for duplicates: 49,683 hash
/// groups is a reviewable number of decisions, 184,058 individual candidates
/// is not.
pub async fn clusters(
    state: &AppState,
    method: Option<&str>,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<Cluster>, AppError> {
    guard(state)?;
    let prefix = match method {
        None | Some("hash") => "hash",
        Some("url") => "url",
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unknown cluster method '{other}'; expected hash or url"
            )))
        }
    };

    // `ovis.doc_dup_group` holds one row per (document, method), so a document
    // that is both a content duplicate and a URL variant appears in both
    // clusters — which is the normal case, and which the single `dup_group`
    // column this replaced could not express: whichever phase ran second
    // evicted the first, and a cluster of three came back with one member and
    // no keeper.
    let rows = sqlx::query(
        "SELECT g.group_key AS key, g.document_id, g.is_keeper, g.group_size, \
                d.semantic_id, d.link, d.chunk_count, \
                COALESCE(d.doc_updated_at, d.last_modified) AS updated_at, \
                pc.id AS candidate_id \
         FROM ovis.doc_dup_group g \
         JOIN public.document d ON d.id = g.document_id \
         LEFT JOIN ovis.prune_candidate pc \
                ON pc.document_id = g.document_id AND pc.state = 'candidate' \
         WHERE g.method = $1 \
           AND ($2::text IS NULL OR g.group_key > $2) \
           AND g.group_key IN ( \
               SELECT group_key FROM ovis.doc_dup_group \
               WHERE method = $1 AND ($2::text IS NULL OR group_key > $2) \
               GROUP BY 1 ORDER BY 1 LIMIT $3) \
         ORDER BY key, g.is_keeper DESC, g.document_id",
    )
    .bind(prefix)
    .bind(after)
    .bind(limit.clamp(1, 100))
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    use sqlx::Row;
    let mut clusters: Vec<Cluster> = Vec::new();
    for row in &rows {
        let key: String = row.get("key");
        let member = ClusterMember {
            document_id: row.get("document_id"),
            semantic_id: row.get("semantic_id"),
            link: row.get("link"),
            chunk_count: row.get("chunk_count"),
            updated_at: row.get("updated_at"),
            is_keeper: row.get("is_keeper"),
            candidate_id: row.get("candidate_id"),
        };
        match clusters.last_mut() {
            Some(cluster) if cluster.key == key => cluster.members.push(member),
            _ => clusters.push(Cluster {
                key,
                method: prefix.to_string(),
                size: row.get::<i32, _>("group_size") as i64,
                members: vec![member],
                keeper_reason: match prefix {
                    "url" => "shortest URL among documents sharing a canonical URL".into(),
                    _ => "shortest URL among documents with identical content".into(),
                },
                narration: None,
            }),
        }
    }
    Ok(clusters)
}

// ---------------------------------------------------------------------------
// Acceptance sampling
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SamplePlan {
    pub population: i64,
    pub sample_size: i64,
    /// Maximum failures in the sample that still supports the claim.
    pub max_failures: i64,
    pub confidence: f64,
    pub max_error_rate: f64,
    pub statement: String,
    pub documents: Vec<SampleDoc>,
}

/// Draw a review sample for a candidate group and state what accepting it
/// would mean.
///
/// The arithmetic is the standard acceptance-sampling bound: with zero
/// failures in `n` independent draws, the true defect rate is below
/// `1 - (1 - c)^(1/n)` at confidence `c`. Stated in a sentence rather than
/// left as a number, because the point is for a human to decide whether that
/// risk is acceptable for this group.
pub async fn sample(
    state: &AppState,
    detector: Option<&str>,
    code: Option<&str>,
    n: i64,
) -> Result<SamplePlan, AppError> {
    guard(state)?;
    let n = n.clamp(1, 200);

    let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "SELECT pc.document_id FROM ovis.prune_candidate pc WHERE pc.state = 'candidate'",
    );
    let mut count_qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "SELECT count(*) FROM ovis.prune_candidate pc WHERE pc.state = 'candidate'",
    );
    for builder in [&mut qb, &mut count_qb] {
        if let Some(detector) = detector {
            builder
                .push(" AND pc.reasons @> ")
                .push_bind(json!([{ "detector": detector }]));
        }
        if let Some(code) = code {
            builder
                .push(" AND pc.reasons @> ")
                .push_bind(json!([{ "code": code }]));
        }
    }
    let population: i64 = count_qb
        .build_query_scalar()
        .fetch_one(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;
    qb.push(" ORDER BY random() LIMIT ").push_bind(n);
    let ids: Vec<String> = qb
        .build_query_scalar()
        .fetch_all(&state.db)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    let sample_size = ids.len() as i64;
    let confidence: f64 = 0.95;
    let max_error_rate = if sample_size > 0 {
        1.0 - (1.0 - confidence).powf(1.0 / sample_size as f64)
    } else {
        1.0
    };

    let docs = documents::documents_by_ids(&state.db, &ids, None).await?;
    let documents: Vec<SampleDoc> = docs
        .into_iter()
        .map(|d| SampleDoc {
            document_id: d.id,
            semantic_id: Some(d.semantic_id),
            chunk_count: d.chunk_count,
            signals: Vec::new(),
        })
        .collect();

    let statement = if sample_size == 0 {
        "This group is empty; there is nothing to sample.".to_string()
    } else {
        format!(
            "Review {sample_size} randomly drawn of {population}. If none of them should be \
             kept, then at 95% confidence fewer than {:.1}% of the group is a mistake — about \
             {} documents. If even one should be kept, tighten this group's threshold instead \
             of staging it.",
            max_error_rate * 100.0,
            (max_error_rate * population as f64).ceil() as i64
        )
    };

    Ok(SamplePlan {
        population,
        sample_size,
        max_failures: 0,
        confidence,
        max_error_rate,
        statement,
        documents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_code_the_detectors_emit_has_a_description() {
        // A code with no description falls through to "Other", which is only
        // correct for user-authored rules whose name *is* the code.
        for code in [
            "exact_duplicate_of",
            "near_duplicate_of",
            "url_variant_of",
            "asset_url",
            "chunkless_stub",
            "thin_content",
            "low_quality_text",
            "lang_not_allowed",
            "stale_content",
            "recrawled_after_prune",
            // Written by `commit`, not by a detector, but they reach the same
            // bundle list and the same card.
            "policy_auto",
            "policy_review",
        ] {
            let (title, description) = describe(code);
            assert_ne!(title, "Other", "{code} needs a description");
            assert!(
                description.len() > 40,
                "{code}'s description must say what the group is: {description}"
            );
        }
    }

    #[test]
    fn tier_and_explicit_policy_are_mutually_exclusive() {
        let err = policy_from_request(Some("standard"), Some(&Policy::aggressive())).unwrap_err();
        assert!(err.to_string().contains("not both"), "{err}");
    }

    #[test]
    fn an_unknown_tier_names_the_valid_ones() {
        let err = policy_from_request(Some("nuclear"), None).unwrap_err();
        assert!(err.to_string().contains("conservative"), "{err}");
        assert!(err.to_string().contains("aggressive"), "{err}");
    }

    #[test]
    fn the_default_is_standard_not_the_most_aggressive_option() {
        let (tier, policy) = policy_from_request(None, None).unwrap();
        assert_eq!(tier.as_deref(), Some("standard"));
        assert_eq!(policy, Policy::standard());
    }

    #[test]
    fn an_invalid_explicit_policy_is_rejected_before_it_can_be_simulated() {
        let mut policy = Policy::standard();
        policy.near_duplicate.auto = Some(2.0);
        let err = policy_from_request(None, Some(&policy)).unwrap_err();
        assert!(err.to_string().contains("between 0 and 1"), "{err}");
    }

    #[test]
    fn policy_hashes_track_content_not_identity() {
        assert_eq!(
            policy_hash(&Policy::standard()),
            policy_hash(&Policy::standard())
        );
        assert_ne!(
            policy_hash(&Policy::standard()),
            policy_hash(&Policy::aggressive())
        );
        assert_eq!(policy_hash(&Policy::standard()).len(), 32);
    }
}

#[cfg(test)]
mod formatting_tests {
    use super::thousands;

    #[test]
    fn digits_are_grouped_for_reading() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(42), "42");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(2_403), "2,403");
        assert_eq!(thousands(1_738_163), "1,738,163");
        assert_eq!(thousands(-1_234), "-1,234");
    }
}
