//! Document profiles: the measurements a scan records, and the policy layer
//! that turns them into bands at read time.
//!
//! The v1 scan baked its thresholds in — a candidate row *was* the verdict, so
//! changing a threshold meant re-scanning 1.7 M documents to find out what the
//! new setting would flag. Here a scan writes `ovis.doc_profile` rows
//! (word counts, gate failures, similarities, URL shape) and a
//! [`Policy`] converts them into `auto` / `review` / untouched bands whenever
//! anyone asks. Simulating a threshold is then a single aggregate query, which
//! is what makes the review UI's live "this level would stage N documents"
//! honest rather than an estimate.
//!
//! Like [`super::prune`], nothing in this module writes an Onyx table; a test
//! at the bottom greps the module's own source to prove it.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use crate::error::CoreResult;

// ---------------------------------------------------------------------------
// The measured profile
// ---------------------------------------------------------------------------

/// Everything one scan measured about one document. Every field is nullable:
/// a scan that ran only the cheap SQL detectors legitimately knows nothing
/// about text quality, and "not measured" must never read as "measured zero".
#[derive(Debug, Clone, Default)]
pub struct DocProfile {
    pub document_id: String,
    pub config_hash: Option<String>,
    pub fingerprint: Option<String>,
    pub connector_id: Option<i32>,
    pub word_count: Option<i32>,
    pub chunk_count: Option<i32>,
    pub quality_metrics: Option<serde_json::Value>,
    pub quality_gates: Option<Vec<String>>,
    pub quality_fail_count: i16,
    pub quality_families: i16,
    pub canonical_url: Option<String>,
    pub url_class: Option<String>,
    pub path_depth: Option<i16>,
    pub has_query: Option<bool>,
    pub archive_of: Option<String>,
    pub lang: Option<String>,
    pub lang_confidence: Option<f32>,
    pub content_hash: Option<String>,
    pub max_jaccard: Option<f32>,
    pub max_jaccard_doc: Option<String>,
    pub max_cosine: Option<f32>,
    pub max_cosine_doc: Option<String>,
    pub centroid_sim: Option<f32>,
    pub centroid_pct: Option<f32>,
    pub cluster_id: Option<i32>,
}

/// Insert or update a batch of profiles in one round trip.
///
/// One statement per page rather than one per document: the v1 scan's exact
/// phase spent ~35 minutes on 21.5k groups almost entirely in per-row upsert
/// latency over the LAN, and profiles are written for *every* document, not
/// just the flagged ones.
pub async fn upsert_profiles(pool: &PgPool, profiles: &[DocProfile]) -> CoreResult<u64> {
    if profiles.is_empty() {
        return Ok(0);
    }
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO ovis.doc_profile (document_id, computed_at, config_hash, fingerprint, \
         connector_id, word_count, chunk_count, quality_metrics, quality_gates, \
         quality_fail_count, quality_families, canonical_url, url_class, path_depth, \
         has_query, archive_of, lang, lang_confidence, content_hash, \
         max_jaccard, max_jaccard_doc, max_cosine, max_cosine_doc, \
         centroid_sim, centroid_pct, cluster_id) ",
    );
    qb.push_values(profiles, |mut row, p| {
        row.push_bind(&p.document_id)
            .push("now()")
            .push_bind(&p.config_hash)
            .push_bind(&p.fingerprint)
            .push_bind(p.connector_id)
            .push_bind(p.word_count)
            .push_bind(p.chunk_count)
            .push_bind(&p.quality_metrics)
            .push_bind(&p.quality_gates)
            .push_bind(p.quality_fail_count)
            .push_bind(p.quality_families)
            .push_bind(&p.canonical_url)
            .push_bind(&p.url_class)
            .push_bind(p.path_depth)
            .push_bind(p.has_query)
            .push_bind(&p.archive_of)
            .push_bind(&p.lang)
            .push_bind(p.lang_confidence)
            .push_bind(&p.content_hash)
            .push_bind(p.max_jaccard)
            .push_bind(&p.max_jaccard_doc)
            .push_bind(p.max_cosine)
            .push_bind(&p.max_cosine_doc)
            .push_bind(p.centroid_sim)
            .push_bind(p.centroid_pct)
            .push_bind(p.cluster_id);
    });
    // COALESCE on the enrichment columns: a cheap scan that re-measures word
    // counts must not erase similarities an expensive scan established.
    //
    // The two quality counters are `NOT NULL DEFAULT 0`, so they cannot say
    // "not measured" the way the nullable columns can — an unqualified
    // assignment let a `thin`-only re-scan write 0 over a real measurement
    // while `quality_gates` (COALESCEd) kept showing the failures, leaving the
    // policy blind to documents whose evidence was still on screen.
    // `quality_gates` is set exactly when the quality detector ran, so it is
    // the honest "was this measured" flag. Assigning rather than GREATEST-ing
    // keeps a genuine re-measure able to lower the count.
    qb.push(
        " ON CONFLICT (document_id) DO UPDATE SET \
           computed_at = now(), \
           config_hash = excluded.config_hash, \
           fingerprint = excluded.fingerprint, \
           connector_id = COALESCE(excluded.connector_id, ovis.doc_profile.connector_id), \
           word_count = COALESCE(excluded.word_count, ovis.doc_profile.word_count), \
           chunk_count = COALESCE(excluded.chunk_count, ovis.doc_profile.chunk_count), \
           quality_metrics = COALESCE(excluded.quality_metrics, ovis.doc_profile.quality_metrics), \
           quality_gates = COALESCE(excluded.quality_gates, ovis.doc_profile.quality_gates), \
           quality_fail_count = CASE WHEN excluded.quality_gates IS NULL \
               THEN ovis.doc_profile.quality_fail_count ELSE excluded.quality_fail_count END, \
           quality_families = CASE WHEN excluded.quality_gates IS NULL \
               THEN ovis.doc_profile.quality_families ELSE excluded.quality_families END, \
           canonical_url = COALESCE(excluded.canonical_url, ovis.doc_profile.canonical_url), \
           url_class = COALESCE(excluded.url_class, ovis.doc_profile.url_class), \
           path_depth = COALESCE(excluded.path_depth, ovis.doc_profile.path_depth), \
           has_query = COALESCE(excluded.has_query, ovis.doc_profile.has_query), \
           archive_of = COALESCE(excluded.archive_of, ovis.doc_profile.archive_of), \
           lang = COALESCE(excluded.lang, ovis.doc_profile.lang), \
           lang_confidence = COALESCE(excluded.lang_confidence, ovis.doc_profile.lang_confidence), \
           content_hash = COALESCE(excluded.content_hash, ovis.doc_profile.content_hash), \
           max_jaccard = GREATEST(excluded.max_jaccard, ovis.doc_profile.max_jaccard), \
           max_jaccard_doc = COALESCE(excluded.max_jaccard_doc, ovis.doc_profile.max_jaccard_doc), \
           max_cosine = GREATEST(excluded.max_cosine, ovis.doc_profile.max_cosine), \
           max_cosine_doc = COALESCE(excluded.max_cosine_doc, ovis.doc_profile.max_cosine_doc), \
           centroid_sim = COALESCE(excluded.centroid_sim, ovis.doc_profile.centroid_sim), \
           centroid_pct = COALESCE(excluded.centroid_pct, ovis.doc_profile.centroid_pct), \
           cluster_id = COALESCE(excluded.cluster_id, ovis.doc_profile.cluster_id)",
    );
    Ok(qb.build().execute(pool).await?.rows_affected())
}

/// Fingerprints of profiles already computed under this config, so a re-scan
/// can skip unchanged documents entirely.
pub async fn profile_fingerprints(
    pool: &PgPool,
    config_hash: &str,
    ids: &[String],
) -> CoreResult<Vec<(String, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT document_id, fingerprint FROM ovis.doc_profile \
         WHERE config_hash = $1 AND fingerprint IS NOT NULL AND document_id = ANY($2)",
    )
    .bind(config_hash)
    .bind(ids.to_vec())
    .fetch_all(pool)
    .await?)
}

/// Document ids sharing each of the given canonical URLs.
pub async fn documents_for_canonical_urls(
    pool: &PgPool,
    keys: &[String],
) -> CoreResult<Vec<(String, String)>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT canonical_url, document_id FROM ovis.doc_profile \
         WHERE canonical_url = ANY($1) ORDER BY canonical_url, document_id",
    )
    .bind(keys.to_vec())
    .fetch_all(pool)
    .await?)
}

/// One document's membership of one duplicate group.
#[derive(Debug, Clone)]
pub struct DupMembership {
    pub document_id: String,
    /// `hash` or `url` — which detector found the group.
    pub method: String,
    /// The content hash or canonical URL the group is keyed by.
    pub group_key: String,
    pub group_size: i32,
    /// The member the policy keeps; every other member is a candidate.
    pub is_keeper: bool,
    /// Whether the group draws members from more than one connector.
    pub cross_connector: bool,
}

/// Record duplicate-group membership for a batch of documents.
///
/// Keyed by `(document_id, method)`, so the exact-duplicate and URL-variant
/// detectors genuinely cannot overwrite each other. They previously shared one
/// column on `doc_profile` and the second phase to run silently evicted the
/// first: a hash cluster of three came back as one member with no keeper, and
/// documents that were still byte-identical stopped matching the
/// `exact_duplicate` signal. A document belonging to both groups is the normal
/// case, not a conflict.
pub async fn set_dup_groups(pool: &PgPool, entries: &[DupMembership]) -> CoreResult<u64> {
    if entries.is_empty() {
        return Ok(0);
    }
    // Postgres refuses an `ON CONFLICT DO UPDATE` that touches the same row
    // twice in one statement, so collapse to one entry per (document, method)
    // first — last write wins, as in `set_max_similarity`.
    let mut latest: std::collections::HashMap<(&str, &str), &DupMembership> =
        std::collections::HashMap::new();
    for entry in entries {
        latest.insert(
            (entry.document_id.as_str(), entry.method.as_str()),
            entry,
        );
    }
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO ovis.doc_dup_group \
             (document_id, method, group_key, group_size, is_keeper, cross_connector) ",
    );
    qb.push_values(latest.values(), |mut row, entry| {
        row.push_bind(&entry.document_id)
            .push_bind(&entry.method)
            .push_bind(&entry.group_key)
            .push_bind(entry.group_size)
            .push_bind(entry.is_keeper)
            .push_bind(entry.cross_connector);
    });
    qb.push(
        " ON CONFLICT (document_id, method) DO UPDATE SET \
            group_key = excluded.group_key, group_size = excluded.group_size, \
            is_keeper = excluded.is_keeper, cross_connector = excluded.cross_connector, \
            computed_at = now()",
    );
    Ok(qb.build().execute(pool).await?.rows_affected())
}

/// Record each document's strongest similarity and the neighbour that produced
/// it. `GREATEST` rather than assignment, so a later page that happens to find
/// a weaker pair cannot lower a document's recorded maximum.
pub async fn set_max_similarity(
    pool: &PgPool,
    method: &str,
    entries: &[(String, f32, String)],
) -> CoreResult<u64> {
    if entries.is_empty() {
        return Ok(0);
    }
    // A document routinely appears in several pairs from one bucket, and
    // Postgres refuses an `ON CONFLICT DO UPDATE` that would touch the same
    // row twice in one statement ("cannot affect row a second time"). Collapse
    // to the strongest pair per document first.
    let mut best: std::collections::HashMap<&str, (f32, &str)> = std::collections::HashMap::new();
    for (id, score, other) in entries {
        let slot = best.entry(id.as_str()).or_insert((*score, other.as_str()));
        if *score > slot.0 {
            *slot = (*score, other.as_str());
        }
    }
    let mut ids: Vec<String> = Vec::with_capacity(best.len());
    let mut scores: Vec<f32> = Vec::with_capacity(best.len());
    let mut others: Vec<String> = Vec::with_capacity(best.len());
    for (id, (score, other)) in best {
        ids.push(id.to_string());
        scores.push(score);
        others.push(other.to_string());
    }
    let sql = match method {
        "minhash" => {
            "INSERT INTO ovis.doc_profile (document_id, max_jaccard, max_jaccard_doc) \
             SELECT * FROM unnest($1::text[], $2::real[], $3::text[]) \
             ON CONFLICT (document_id) DO UPDATE SET \
               max_jaccard = GREATEST(excluded.max_jaccard, ovis.doc_profile.max_jaccard), \
               max_jaccard_doc = CASE \
                 WHEN excluded.max_jaccard >= COALESCE(ovis.doc_profile.max_jaccard, -1) \
                 THEN excluded.max_jaccard_doc ELSE ovis.doc_profile.max_jaccard_doc END"
        }
        "cosine" => {
            "INSERT INTO ovis.doc_profile (document_id, max_cosine, max_cosine_doc) \
             SELECT * FROM unnest($1::text[], $2::real[], $3::text[]) \
             ON CONFLICT (document_id) DO UPDATE SET \
               max_cosine = GREATEST(excluded.max_cosine, ovis.doc_profile.max_cosine), \
               max_cosine_doc = CASE \
                 WHEN excluded.max_cosine >= COALESCE(ovis.doc_profile.max_cosine, -1) \
                 THEN excluded.max_cosine_doc ELSE ovis.doc_profile.max_cosine_doc END"
        }
        other => {
            return Err(crate::error::CoreError::Invalid(format!(
                "unknown similarity method '{other}'"
            )))
        }
    };
    Ok(sqlx::query(sql)
        .bind(&ids)
        .bind(&scores)
        .bind(&others)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn profile_count(pool: &PgPool) -> CoreResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM ovis.doc_profile")
        .fetch_one(pool)
        .await?)
}

/// Documents sharing a canonical URL with at least one other document —
/// the URL-variant duplicate groups.
pub async fn canonical_url_groups(
    pool: &PgPool,
    after: Option<&str>,
    limit: i64,
) -> CoreResult<Vec<(String, i64)>> {
    let rows = sqlx::query(
        "SELECT canonical_url, count(*) AS members FROM ovis.doc_profile \
         WHERE canonical_url IS NOT NULL AND ($1::text IS NULL OR canonical_url > $1) \
         GROUP BY canonical_url HAVING count(*) > 1 \
         ORDER BY canonical_url LIMIT $2",
    )
    .bind(after)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("canonical_url"), r.get("members")))
        .collect())
}

// ---------------------------------------------------------------------------
// Verified pairs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DupPair {
    pub a: String,
    pub b: String,
    pub method: String,
    pub estimated: Option<f32>,
    pub verified: Option<f32>,
    pub cosine: Option<f32>,
    pub same_connector: Option<bool>,
}

/// Store verified pair similarities in one round trip. Pairs are stored with
/// `a < b` so the same pair never lands twice under two orderings.
pub async fn upsert_pairs(pool: &PgPool, pairs: &[DupPair]) -> CoreResult<u64> {
    if pairs.is_empty() {
        return Ok(0);
    }
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO ovis.dup_pair (a, b, method, estimated, verified, cosine, same_connector) ",
    );
    qb.push_values(pairs, |mut row, p| {
        let (a, b) = if p.a <= p.b {
            (&p.a, &p.b)
        } else {
            (&p.b, &p.a)
        };
        row.push_bind(a)
            .push_bind(b)
            .push_bind(&p.method)
            .push_bind(p.estimated)
            .push_bind(p.verified)
            .push_bind(p.cosine)
            .push_bind(p.same_connector);
    });
    qb.push(
        " ON CONFLICT (a, b, method) DO UPDATE SET \
           estimated = COALESCE(excluded.estimated, ovis.dup_pair.estimated), \
           verified = COALESCE(excluded.verified, ovis.dup_pair.verified), \
           cosine = COALESCE(excluded.cosine, ovis.dup_pair.cosine), \
           same_connector = COALESCE(excluded.same_connector, ovis.dup_pair.same_connector), \
           verified_at = now()",
    );
    Ok(qb.build().execute(pool).await?.rows_affected())
}

/// Every stored pair involving this document, strongest first.
pub async fn pairs_for_document(
    pool: &PgPool,
    document_id: &str,
    limit: i64,
) -> CoreResult<Vec<(String, String, f32, Option<f32>)>> {
    let rows = sqlx::query(
        "SELECT a, b, method, COALESCE(verified, estimated) AS score, cosine \
         FROM ovis.dup_pair WHERE a = $1 OR b = $1 \
         ORDER BY COALESCE(verified, estimated) DESC NULLS LAST LIMIT $2",
    )
    .bind(document_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let a: String = r.get("a");
            let b: String = r.get("b");
            let other = if a == document_id { b } else { a };
            (
                other,
                r.get::<String, _>("method"),
                r.get::<Option<f32>, _>("score").unwrap_or(0.0),
                r.get::<Option<f32>, _>("cosine"),
            )
        })
        .collect())
}

pub async fn pair_count(pool: &PgPool) -> CoreResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM ovis.dup_pair")
        .fetch_one(pool)
        .await?)
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// What a policy decides for one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    /// Strong enough to stage in bulk after a sampled check.
    Auto,
    /// Surfaced for human review.
    Review,
    /// Left alone.
    None,
}

impl Band {
    pub fn code(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Review => "review",
            Self::None => "none",
        }
    }
}

/// A threshold with two levels. `auto` is the stronger of the two; a signal
/// whose `auto` is `None` can only ever produce review candidates, which is
/// how quality gates and off-topic scores are kept out of bulk staging.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Threshold {
    pub auto: Option<f64>,
    pub review: Option<f64>,
}

/// Quality gates count failures rather than crossing a similarity, so they get
/// their own shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct QualityThreshold {
    /// Minimum failing gates to reach the review band. `None` disables.
    pub review_min_failures: Option<i64>,
    pub min_families: i64,
    /// Quality heuristics never auto-stage: they identify *unusual*
    /// documents, and unusual overlaps with valuable (API references, data
    /// tables). Present so the shape is uniform, defaulted off, and the UI
    /// says so.
    pub auto_min_failures: Option<i64>,
}

impl Default for QualityThreshold {
    fn default() -> Self {
        Self {
            review_min_failures: Some(3),
            min_families: 2,
            auto_min_failures: None,
        }
    }
}

/// A named set of thresholds. Applying one is what creates candidates;
/// simulating one changes nothing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Identical `content_hash`, excess copies beyond the keeper.
    pub exact_duplicate: Band,
    /// Same canonical URL, excess copies beyond the keeper.
    pub url_variant: Band,
    /// Image/media/archive URLs whose text is a crawl artefact.
    pub asset: Band,
    /// Zero-chunk stubs past the age gate.
    pub stub: Band,
    /// Verified MinHash Jaccard.
    pub near_duplicate: Threshold,
    /// Embedding cosine.
    pub semantic: Threshold,
    /// Text quality gates.
    pub quality: QualityThreshold,
    /// Percentile of similarity-to-connector-centroid, below which a document
    /// is surfaced. Review-only by construction.
    pub off_topic_percentile: Option<f64>,
    /// Connectors this policy never touches.
    pub exempt_connectors: Vec<String>,
    /// Cross-connector duplicates are held to the stricter band. FineWeb's
    /// finding is that global dedup over-prunes: a document mirrored across
    /// sources is often popular rather than junk.
    pub cross_connector_review_only: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self::standard()
    }
}

impl Policy {
    /// Only what is provably redundant: byte-identical copies and empty stubs.
    pub fn conservative() -> Self {
        Self {
            exact_duplicate: Band::Auto,
            url_variant: Band::Review,
            asset: Band::Review,
            stub: Band::Auto,
            near_duplicate: Threshold {
                auto: Some(0.95),
                review: Some(0.85),
            },
            semantic: Threshold {
                auto: None,
                review: Some(0.97),
            },
            quality: QualityThreshold {
                review_min_failures: Some(4),
                min_families: 3,
                auto_min_failures: None,
            },
            off_topic_percentile: None,
            exempt_connectors: Vec::new(),
            cross_connector_review_only: true,
        }
    }

    /// Adds verified near-duplicates and multi-gate quality failures.
    pub fn standard() -> Self {
        Self {
            exact_duplicate: Band::Auto,
            url_variant: Band::Auto,
            asset: Band::Auto,
            stub: Band::Auto,
            near_duplicate: Threshold {
                auto: Some(0.90),
                review: Some(0.80),
            },
            semantic: Threshold {
                auto: None,
                review: Some(0.93),
            },
            quality: QualityThreshold::default(),
            off_topic_percentile: None,
            exempt_connectors: Vec::new(),
            cross_connector_review_only: true,
        }
    }

    /// Adds semantic duplicates, paraphrase-level matches and off-topic
    /// outliers. Expect false positives; the boundary sample is the check.
    pub fn aggressive() -> Self {
        Self {
            exact_duplicate: Band::Auto,
            url_variant: Band::Auto,
            asset: Band::Auto,
            stub: Band::Auto,
            near_duplicate: Threshold {
                auto: Some(0.85),
                review: Some(0.75),
            },
            semantic: Threshold {
                auto: Some(0.97),
                review: Some(0.88),
            },
            quality: QualityThreshold {
                review_min_failures: Some(2),
                min_families: 2,
                auto_min_failures: None,
            },
            off_topic_percentile: Some(1.0),
            exempt_connectors: Vec::new(),
            cross_connector_review_only: true,
        }
    }

    pub fn by_tier(tier: &str) -> Option<Self> {
        match tier {
            "conservative" => Some(Self::conservative()),
            "standard" => Some(Self::standard()),
            "aggressive" => Some(Self::aggressive()),
            _ => None,
        }
    }

    /// Reject a policy that cannot mean what it says, rather than silently
    /// evaluating to nothing.
    pub fn validate(&self) -> Result<(), String> {
        for (name, t) in [("near_duplicate", &self.near_duplicate), ("semantic", &self.semantic)] {
            for (level, value) in [("auto", t.auto), ("review", t.review)] {
                if let Some(v) = value {
                    if !(0.0..=1.0).contains(&v) {
                        return Err(format!("{name}.{level} must be between 0 and 1, got {v}"));
                    }
                }
            }
            if let (Some(auto), Some(review)) = (t.auto, t.review) {
                if auto < review {
                    return Err(format!(
                        "{name}.auto ({auto}) must be at least {name}.review ({review}); \
                         the auto band is the stronger claim"
                    ));
                }
            }
        }
        if let Some(pct) = self.off_topic_percentile {
            if !(0.0..=50.0).contains(&pct) {
                return Err(format!(
                    "off_topic_percentile is a bottom-percentile cut and must be between 0 and 50, got {pct}"
                ));
            }
        }
        if let (Some(auto), Some(review)) = (
            self.quality.auto_min_failures,
            self.quality.review_min_failures,
        ) {
            if auto < review {
                return Err(format!(
                    "quality.auto_min_failures ({auto}) must be at least \
                     quality.review_min_failures ({review})"
                ));
            }
        }
        Ok(())
    }
}

/// One signal's SQL predicate, with the band it grants.
struct SignalSql {
    signal: &'static str,
    band: Band,
    /// The predicate as this band applies it, narrowed by any guard.
    sql: String,
    /// The same predicate without the guard. The review band is built from
    /// these, so a document a guard held back from `auto` still reaches
    /// `review` rather than falling out of the policy entirely.
    base: String,
}

/// Build the per-signal predicates for a policy.
///
/// Column names are compile-time constants; only thresholds are bound, so
/// nothing here can be injected through a policy body.
fn signal_predicates(policy: &Policy) -> Vec<SignalSql> {
    let mut out = Vec::new();

    // A duplicate whose group spans connectors is held back from the bulk
    // band when `cross_connector_review_only` is set: it still reaches
    // review through the plain clause below, it just stops being something
    // the auto band stages without a human. FineWeb's finding is that global
    // dedup over-prunes — a document mirrored across sources is usually
    // popular rather than redundant.
    //
    // The group side reads a flag the scan recorded, so the check costs
    // nothing at read time. The similarity side probes `dup_pair` for the
    // neighbour that produced the recorded maximum; pairs are stored with
    // `a < b`, so the lookup is a primary-key probe, and it only runs for
    // rows that already cleared the threshold.
    // A duplicate group the document is a *member* of — never the keeper, which
    // is the copy the policy is choosing to survive.
    let in_group = |method: &str, guarded: bool| {
        format!(
            "EXISTS (SELECT 1 FROM ovis.doc_dup_group g \
               WHERE g.document_id = p.document_id AND g.method = '{method}' \
                 AND NOT g.is_keeper AND g.group_size > 1{})",
            if guarded { " AND NOT g.cross_connector" } else { "" }
        )
    };
    let pair_same_connector = |neighbour: &str| {
        format!(
            "NOT EXISTS (SELECT 1 FROM ovis.dup_pair dp \
               WHERE dp.a = LEAST(p.document_id, {neighbour}) \
                 AND dp.b = GREATEST(p.document_id, {neighbour}) \
                 AND dp.same_connector IS FALSE)"
        )
    };

    let mut flag = |signal: &'static str, band: Band, sql: &str, guarded: Option<&str>| {
        match (band, guarded) {
            (Band::None, _) => {}
            // Auto, but the policy holds cross-connector copies to review:
            // narrow the auto clause and keep the unguarded one at review, so
            // the breakdown still attributes those documents to this signal.
            (Band::Auto, Some(guarded)) if policy.cross_connector_review_only => {
                out.push(SignalSql {
                    signal,
                    band: Band::Auto,
                    sql: guarded.to_string(),
                    base: sql.to_string(),
                });
                out.push(SignalSql {
                    signal,
                    band: Band::Review,
                    sql: sql.to_string(),
                    base: sql.to_string(),
                });
            }
            _ => out.push(SignalSql {
                signal,
                band,
                sql: sql.to_string(),
                base: sql.to_string(),
            }),
        }
    };

    flag(
        "exact_duplicate",
        policy.exact_duplicate,
        &in_group("hash", false),
        Some(&in_group("hash", true)),
    );
    flag(
        "url_variant",
        policy.url_variant,
        &in_group("url", false),
        Some(&in_group("url", true)),
    );
    flag(
        "asset",
        policy.asset,
        "(p.url_class IN ('image','media','archive') AND COALESCE(p.chunk_count, 0) <= 1)",
        None,
    );
    flag("stub", policy.stub, "(p.chunk_count = 0)", None);

    for (signal, column, neighbour, threshold) in [
        (
            "near_duplicate",
            "p.max_jaccard",
            "p.max_jaccard_doc",
            &policy.near_duplicate,
        ),
        (
            "semantic",
            "p.max_cosine",
            "p.max_cosine_doc",
            &policy.semantic,
        ),
    ] {
        if let Some(auto) = threshold.auto {
            let base = format!("({column} >= {auto})");
            let guarded = format!("({base} AND {})", pair_same_connector(neighbour));
            flag(signal, Band::Auto, &base, Some(&guarded));
        }
        if let Some(review) = threshold.review {
            flag(signal, Band::Review, &format!("({column} >= {review})"), None);
        }
    }

    if let Some(min) = policy.quality.review_min_failures {
        flag(
            "quality",
            Band::Review,
            &format!(
                "(p.quality_fail_count >= {min} AND p.quality_families >= {})",
                policy.quality.min_families
            ),
            None,
        );
    }
    if let Some(min) = policy.quality.auto_min_failures {
        flag(
            "quality",
            Band::Auto,
            &format!(
                "(p.quality_fail_count >= {min} AND p.quality_families >= {})",
                policy.quality.min_families
            ),
            None,
        );
    }
    if let Some(pct) = policy.off_topic_percentile {
        flag(
            "off_topic",
            Band::Review,
            &format!("(p.centroid_pct IS NOT NULL AND p.centroid_pct <= {pct})"),
            None,
        );
    }
    out
}

/// `OR` of every predicate granting at least `band`.
///
/// Wrapped in `COALESCE(…, FALSE)` because most profile columns are nullable
/// and SQL comparison against NULL yields NULL, not FALSE. Without it,
/// `max_jaccard >= 0.9` on an unmeasured document makes the whole OR NULL, and
/// then `NOT (auto)` is NULL too — so the review count silently drops every
/// document that has not been measured by *every* signal in the policy. That
/// reads as "this policy would do nothing" rather than as a missing
/// measurement, which is the most misleading answer the simulation could give.
fn band_predicate(policy: &Policy, band: Band) -> String {
    let mut parts: Vec<String> = Vec::new();
    for signal in signal_predicates(policy) {
        let part = match band {
            Band::Auto if signal.band == Band::Auto => signal.sql,
            Band::Auto => continue,
            // Anything that reaches auto also reaches review — and it reaches
            // it *unguarded*, so a cross-connector duplicate held back from
            // the bulk band still lands in review rather than nowhere.
            Band::Review => signal.base,
            Band::None => continue,
        };
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        "FALSE".to_string()
    } else {
        format!("COALESCE({}, FALSE)", parts.join(" OR "))
    }
}

/// Documents a policy never touches, whatever it measured.
fn exemption_predicate(policy: &Policy) -> String {
    let mut clauses = vec![
        // Never re-flag a document with an open lifecycle row or an exclusion.
        "NOT EXISTS (SELECT 1 FROM ovis.prune_candidate pc WHERE pc.document_id = p.document_id \
         AND pc.state IN ('candidate','staged','deleting'))"
            .to_string(),
        "NOT EXISTS (SELECT 1 FROM ovis.prune_exclusions e WHERE e.document_id = p.document_id)"
            .to_string(),
        // The document must still exist.
        "EXISTS (SELECT 1 FROM public.document d WHERE d.id = p.document_id)".to_string(),
    ];
    if !policy.exempt_connectors.is_empty() {
        let escaped: Vec<String> = policy
            .exempt_connectors
            .iter()
            .map(|c| format!("'{}'", c.replace('\'', "''")))
            .collect();
        clauses.push(format!(
            "NOT EXISTS (SELECT 1 FROM public.connector c WHERE c.id = p.connector_id \
             AND c.name IN ({}))",
            escaped.join(", ")
        ));
    }
    clauses.join(" AND ")
}

/// What a policy would do, without doing it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimulationResult {
    /// Documents with a profile at all.
    pub profiled: i64,
    pub auto: i64,
    pub review: i64,
    pub untouched: i64,
    /// Per-signal counts, in band order. A document can appear under several.
    pub by_signal: Vec<SignalCount>,
    pub by_connector: Vec<ConnectorCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalCount {
    pub signal: String,
    pub band: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorCount {
    pub connector_id: Option<i32>,
    pub connector_name: Option<String>,
    pub auto: i64,
    pub review: i64,
}

/// Evaluate a policy against the stored profiles. Reads only; nothing is
/// created, which is what makes this safe to run on every slider drag.
pub async fn simulate(pool: &PgPool, policy: &Policy) -> CoreResult<SimulationResult> {
    let auto = band_predicate(policy, Band::Auto);
    let review = band_predicate(policy, Band::Review);
    let exempt = exemption_predicate(policy);

    let row = sqlx::query(&format!(
        "SELECT count(*) AS profiled, \
                count(*) FILTER (WHERE {exempt} AND {auto}) AS auto_count, \
                count(*) FILTER (WHERE {exempt} AND {review} AND NOT ({auto})) AS review_count \
         FROM ovis.doc_profile p"
    ))
    .fetch_one(pool)
    .await?;

    let profiled: i64 = row.get("profiled");
    let auto_count: i64 = row.get("auto_count");
    let review_count: i64 = row.get("review_count");

    let mut by_signal = Vec::new();
    for signal in signal_predicates(policy) {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM ovis.doc_profile p WHERE {exempt} AND COALESCE({}, FALSE)",
            signal.sql
        ))
        .fetch_one(pool)
        .await?;
        if count > 0 {
            by_signal.push(SignalCount {
                signal: signal.signal.to_string(),
                band: signal.band.code().to_string(),
                count,
            });
        }
    }
    by_signal.sort_by_key(|s| std::cmp::Reverse(s.count));

    let connector_rows = sqlx::query(&format!(
        "SELECT p.connector_id, c.name AS connector_name, \
                count(*) FILTER (WHERE {auto}) AS auto_count, \
                count(*) FILTER (WHERE {review} AND NOT ({auto})) AS review_count \
         FROM ovis.doc_profile p \
         LEFT JOIN public.connector c ON c.id = p.connector_id \
         WHERE {exempt} AND {review} \
         GROUP BY p.connector_id, c.name \
         ORDER BY count(*) DESC LIMIT 50"
    ))
    .fetch_all(pool)
    .await?;

    Ok(SimulationResult {
        profiled,
        auto: auto_count,
        review: review_count,
        untouched: profiled - auto_count - review_count,
        by_signal,
        by_connector: connector_rows
            .into_iter()
            .map(|r| ConnectorCount {
                connector_id: r.get("connector_id"),
                connector_name: r.get("connector_name"),
                auto: r.get("auto_count"),
                review: r.get("review_count"),
            })
            .collect(),
    })
}

/// Document ids a policy would put in a band, for candidate creation and for
/// the boundary sampler.
pub async fn documents_in_band(
    pool: &PgPool,
    policy: &Policy,
    band: Band,
    after: Option<&str>,
    limit: i64,
) -> CoreResult<Vec<String>> {
    let auto = band_predicate(policy, Band::Auto);
    let review = band_predicate(policy, Band::Review);
    let exempt = exemption_predicate(policy);
    let predicate = match band {
        Band::Auto => auto.clone(),
        Band::Review => format!("{review} AND NOT ({auto})"),
        Band::None => return Ok(Vec::new()),
    };
    Ok(sqlx::query_scalar(&format!(
        "SELECT p.document_id FROM ovis.doc_profile p \
         WHERE {exempt} AND ({predicate}) AND ($1::text IS NULL OR p.document_id > $1) \
         ORDER BY p.document_id LIMIT $2"
    ))
    .bind(after)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// A random sample from a band — the acceptance-sampling draw. Server-side
/// randomness so a client cannot cherry-pick an easy sample.
pub async fn sample_band(
    pool: &PgPool,
    policy: &Policy,
    band: Band,
    n: i64,
) -> CoreResult<Vec<String>> {
    let auto = band_predicate(policy, Band::Auto);
    let review = band_predicate(policy, Band::Review);
    let exempt = exemption_predicate(policy);
    let predicate = match band {
        Band::Auto => auto.clone(),
        Band::Review => format!("{review} AND NOT ({auto})"),
        Band::None => return Ok(Vec::new()),
    };
    Ok(sqlx::query_scalar(&format!(
        "SELECT p.document_id FROM ovis.doc_profile p \
         WHERE {exempt} AND ({predicate}) ORDER BY random() LIMIT $1"
    ))
    .bind(n)
    .fetch_all(pool)
    .await?)
}

/// Which signals fired for each of `ids` — the batched form of
/// [`signals_for_document`], and the one every bulk path uses.
///
/// One statement evaluates every predicate for a whole page as boolean
/// columns. The per-document form costs one round trip *per signal per
/// document*: committing a band of 200k documents under a policy with ten
/// active signals meant two million queries, which is not a slow commit but an
/// unusable one.
pub async fn signals_for_documents(
    pool: &PgPool,
    policy: &Policy,
    ids: &[String],
) -> CoreResult<std::collections::HashMap<String, Vec<(String, String)>>> {
    let mut out: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let signals = signal_predicates(policy);
    if signals.is_empty() {
        return Ok(out);
    }
    let columns: Vec<String> = signals
        .iter()
        .enumerate()
        .map(|(i, s)| format!("COALESCE({}, FALSE) AS s{i}", s.sql))
        .collect();
    let rows = sqlx::query(&format!(
        "SELECT p.document_id, {} FROM ovis.doc_profile p WHERE p.document_id = ANY($1)",
        columns.join(", ")
    ))
    .bind(ids.to_vec())
    .fetch_all(pool)
    .await?;

    for row in &rows {
        let document_id: String = row.get("document_id");
        let mut hits: Vec<(String, String)> = Vec::new();
        for (i, signal) in signals.iter().enumerate() {
            // The strongest band wins, as in `signals_for_document`.
            if hits.iter().any(|(name, band)| name == signal.signal && band == "auto") {
                continue;
            }
            if row.get::<Option<bool>, _>(format!("s{i}").as_str()) == Some(true) {
                hits.retain(|(name, _)| name != signal.signal || signal.band != Band::Auto);
                hits.push((signal.signal.to_string(), signal.band.code().to_string()));
            }
        }
        if !hits.is_empty() {
            out.insert(document_id, hits);
        }
    }
    Ok(out)
}

/// Which signals fired for one document under a policy — the evidence the
/// review UI shows next to a candidate.
pub async fn signals_for_document(
    pool: &PgPool,
    policy: &Policy,
    document_id: &str,
) -> CoreResult<Vec<(String, String)>> {
    let mut hits: Vec<(String, String)> = Vec::new();
    for signal in signal_predicates(policy) {
        // One signal contributes an auto and a review clause, and anything
        // that clears auto clears review too. Reporting both would read as two
        // independent findings, so the strongest band wins and the weaker one
        // is dropped.
        if hits.iter().any(|(name, band)| name == signal.signal && band == "auto") {
            continue;
        }
        let matched: Option<bool> = sqlx::query_scalar(&format!(
            "SELECT COALESCE({}, FALSE) FROM ovis.doc_profile p WHERE p.document_id = $1",
            signal.sql
        ))
        .bind(document_id)
        .fetch_optional(pool)
        .await?
        .flatten();
        if matched == Some(true) {
            hits.retain(|(name, _)| name != signal.signal || signal.band != Band::Auto);
            hits.push((signal.signal.to_string(), signal.band.code().to_string()));
        }
    }
    Ok(hits)
}

// ---------------------------------------------------------------------------
// Histograms
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramBucket {
    pub lower: f64,
    pub upper: f64,
    pub count: i64,
}

/// The signals the UI can draw a distribution for.
pub const HISTOGRAM_SIGNALS: [&str; 5] = [
    "max_jaccard",
    "max_cosine",
    "quality_fail_count",
    "word_count",
    "centroid_pct",
];

/// Bucketed distribution of one profile column — what the threshold dial is
/// drawn from. Similarity columns bucket over 0–1; the others over their own
/// measured range.
pub async fn histogram(pool: &PgPool, signal: &str, buckets: i64) -> CoreResult<Vec<HistogramBucket>> {
    if !HISTOGRAM_SIGNALS.contains(&signal) {
        return Err(crate::error::CoreError::Invalid(format!(
            "unknown histogram signal '{signal}'; expected one of {}",
            HISTOGRAM_SIGNALS.join(", ")
        )));
    }
    let buckets = buckets.clamp(2, 100);
    let (lo, hi): (f64, f64) = match signal {
        "max_jaccard" | "max_cosine" => (0.0, 1.0),
        "centroid_pct" => (0.0, 100.0),
        _ => {
            let row = sqlx::query(&format!(
                "SELECT COALESCE(min({signal}), 0)::float8 AS lo, \
                        COALESCE(max({signal}), 0)::float8 AS hi \
                 FROM ovis.doc_profile WHERE {signal} IS NOT NULL"
            ))
            .fetch_one(pool)
            .await?;
            (row.get("lo"), row.get("hi"))
        }
    };
    if hi <= lo {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(&format!(
        "SELECT width_bucket({signal}::float8, $1, $2, $3) AS bucket, count(*) AS n \
         FROM ovis.doc_profile WHERE {signal} IS NOT NULL \
         GROUP BY bucket ORDER BY bucket"
    ))
    .bind(lo)
    .bind(hi)
    .bind(buckets as i32)
    .fetch_all(pool)
    .await?;

    let width = (hi - lo) / buckets as f64;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let bucket: Option<i32> = r.get("bucket");
            let bucket = bucket?;
            // width_bucket returns 0 for below-range and buckets+1 for above;
            // clamp both into the visible range rather than dropping them.
            let idx = (bucket.max(1) as i64).min(buckets) - 1;
            Some(HistogramBucket {
                lower: lo + idx as f64 * width,
                upper: lo + (idx + 1) as f64 * width,
                count: r.get::<i64, _>("n"),
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Stored policies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPolicy {
    pub id: i64,
    pub name: String,
    pub tier: String,
    pub body: serde_json::Value,
    pub config_hash: String,
    pub active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn row_to_policy(r: &sqlx::postgres::PgRow) -> StoredPolicy {
    StoredPolicy {
        id: r.get("id"),
        name: r.get("name"),
        tier: r.get("tier"),
        body: r.get("body"),
        config_hash: r.get("config_hash"),
        active: r.get("active"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

const POLICY_COLUMNS: &str =
    "SELECT id, name, tier, body, config_hash, active, created_at, updated_at FROM ovis.prune_policy";

pub async fn list_policies(pool: &PgPool) -> CoreResult<Vec<StoredPolicy>> {
    let rows = sqlx::query(&format!("{POLICY_COLUMNS} ORDER BY name"))
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_policy).collect())
}

pub async fn get_policy(pool: &PgPool, name: &str) -> CoreResult<Option<StoredPolicy>> {
    let row = sqlx::query(&format!("{POLICY_COLUMNS} WHERE name = $1"))
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_policy))
}

pub async fn active_policy(pool: &PgPool) -> CoreResult<Option<StoredPolicy>> {
    let row = sqlx::query(&format!("{POLICY_COLUMNS} WHERE active ORDER BY updated_at DESC LIMIT 1"))
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_policy))
}

pub async fn save_policy(
    pool: &PgPool,
    name: &str,
    tier: &str,
    body: &serde_json::Value,
    config_hash: &str,
) -> CoreResult<StoredPolicy> {
    let row = sqlx::query(
        "INSERT INTO ovis.prune_policy (name, tier, body, config_hash) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (name) DO UPDATE SET tier = excluded.tier, body = excluded.body, \
             config_hash = excluded.config_hash, updated_at = now() \
         RETURNING id, name, tier, body, config_hash, active, created_at, updated_at",
    )
    .bind(name)
    .bind(tier)
    .bind(body)
    .bind(config_hash)
    .fetch_one(pool)
    .await?;
    Ok(row_to_policy(&row))
}

/// Make one policy active, deactivating the rest. Exactly one policy is the
/// standing answer to "what does this deployment consider prunable".
pub async fn activate_policy(pool: &PgPool, name: &str) -> CoreResult<bool> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE ovis.prune_policy SET active = false WHERE active")
        .execute(&mut *tx)
        .await?;
    let updated = sqlx::query(
        "UPDATE ovis.prune_policy SET active = true, updated_at = now() WHERE name = $1",
    )
    .bind(name)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(updated == 1)
}

pub async fn delete_policy(pool: &PgPool, name: &str) -> CoreResult<bool> {
    let deleted = sqlx::query("DELETE FROM ovis.prune_policy WHERE name = $1")
        .bind(name)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(deleted == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_write_statement_in_this_module_targets_an_onyx_table() {
        let source = include_str!("profile.rs");
        for (idx, line) in source.lines().enumerate() {
            let lowered = line.to_lowercase();
            for verb in ["insert into ", "update ", "delete from "] {
                if let Some(pos) = lowered.find(verb) {
                    let target = lowered[pos + verb.len()..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("");
                    assert!(
                        !target.starts_with("public."),
                        "line {}: profile layer writes to an Onyx table: {}",
                        idx + 1,
                        line.trim()
                    );
                }
            }
        }
    }

    #[test]
    fn presets_are_ordered_from_conservative_to_aggressive() {
        let c = Policy::conservative();
        let s = Policy::standard();
        let a = Policy::aggressive();
        // A lower near-duplicate threshold catches strictly more.
        assert!(c.near_duplicate.auto > s.near_duplicate.auto);
        assert!(s.near_duplicate.auto > a.near_duplicate.auto);
        assert!(c.quality.review_min_failures > s.quality.review_min_failures);
        assert!(s.quality.review_min_failures > a.quality.review_min_failures);
        assert!(c.off_topic_percentile.is_none());
        assert!(a.off_topic_percentile.is_some());
    }

    #[test]
    fn quality_gates_never_auto_stage_in_any_shipped_preset() {
        for policy in [Policy::conservative(), Policy::standard(), Policy::aggressive()] {
            assert!(
                policy.quality.auto_min_failures.is_none(),
                "text heuristics identify unusual documents, not worthless ones"
            );
        }
    }

    #[test]
    fn validation_rejects_an_auto_band_weaker_than_its_review_band() {
        let mut policy = Policy::standard();
        policy.near_duplicate = Threshold {
            auto: Some(0.5),
            review: Some(0.9),
        };
        let err = policy.validate().unwrap_err();
        assert!(err.contains("near_duplicate"), "{err}");
        assert!(err.contains("stronger claim"), "{err}");
    }

    #[test]
    fn validation_rejects_out_of_range_similarities() {
        let mut policy = Policy::standard();
        policy.semantic = Threshold {
            auto: Some(1.5),
            review: None,
        };
        assert!(policy.validate().unwrap_err().contains("between 0 and 1"));
        let mut policy = Policy::standard();
        policy.off_topic_percentile = Some(90.0);
        assert!(policy.validate().unwrap_err().contains("bottom-percentile"));
    }

    #[test]
    fn every_shipped_preset_validates() {
        for policy in [Policy::conservative(), Policy::standard(), Policy::aggressive()] {
            policy.validate().expect("shipped presets must be valid");
        }
    }

    #[test]
    fn the_auto_predicate_is_a_subset_of_the_review_predicate() {
        // Structurally: every auto clause implies a review clause, so anything
        // auto also satisfies review. The simulation relies on this to compute
        // the review count as "review AND NOT auto" without double counting.
        //
        // A cross-connector guard narrows the auto clause to `(base AND
        // guard)` while review keeps the bare `base`, so the check strips a
        // trailing guard before looking for the clause.
        for policy in [Policy::conservative(), Policy::standard(), Policy::aggressive()] {
            let auto = band_predicate(&policy, Band::Auto);
            let review = band_predicate(&policy, Band::Review);
            for signal in signal_predicates(&policy) {
                if signal.band != Band::Auto {
                    continue;
                }
                assert!(
                    auto.contains(&signal.sql),
                    "auto predicate is missing {}: {auto}",
                    signal.sql
                );
                assert!(
                    review.contains(&signal.base),
                    "review predicate is missing the auto clause for {} ({}): {review}",
                    signal.signal,
                    signal.base
                );
            }
        }
    }

    /// The cross-connector rule is a behaviour, not a stored preference.
    ///
    /// It shipped as a field every preset set and no predicate read, so a
    /// policy that claimed to hold mirrored copies back from bulk staging
    /// staged them anyway. These assertions are on the generated SQL because
    /// that is the only place the setting can have an effect.
    #[test]
    fn cross_connector_duplicates_are_kept_out_of_the_bulk_band() {
        let mut policy = Policy::standard();
        policy.cross_connector_review_only = true;
        let auto = band_predicate(&policy, Band::Auto);
        let review = band_predicate(&policy, Band::Review);

        assert!(
            auto.contains("NOT g.cross_connector"),
            "the duplicate-group auto band must exclude cross-connector groups: {auto}"
        );
        assert!(
            auto.contains("ovis.dup_pair"),
            "the similarity auto band must exclude cross-connector pairs: {auto}"
        );
        // Held back, not dropped: review keeps the *unguarded* clause, so a
        // mirrored copy is still surfaced for a human rather than vanishing
        // from the policy altogether.
        assert!(
            !review.contains("cross_connector") && !review.contains("ovis.dup_pair"),
            "the review band must not carry the guard: {review}"
        );
        assert!(
            review.contains("g.method = 'hash'") && review.contains("p.max_jaccard >= 0.9"),
            "cross-connector duplicates must still reach the review band: {review}"
        );
        // The keeper is the copy the policy is choosing to survive, so it must
        // never be selectable by any band.
        assert!(
            review.contains("NOT g.is_keeper"),
            "a group's keeper must never be flagged: {review}"
        );

        policy.cross_connector_review_only = false;
        let auto = band_predicate(&policy, Band::Auto);
        assert!(
            !auto.contains("cross_connector") && !auto.contains("ovis.dup_pair"),
            "turning the rule off must remove the guard entirely: {auto}"
        );
    }

    /// Signals that grant no band at all are not narrowed by the guard: a
    /// review-only threshold is already the cautious answer.
    #[test]
    fn the_cross_connector_guard_only_narrows_the_bulk_band() {
        let mut policy = Policy::conservative();
        policy.semantic = Threshold {
            auto: None,
            review: Some(0.9),
        };
        let review = band_predicate(&policy, Band::Review);
        assert!(review.contains("p.max_cosine >= 0.9"), "{review}");
        assert!(
            !review.contains("dup_pair dp \n"),
            "a review-only threshold needs no pair probe: {review}"
        );
    }

    /// Every band predicate must be NULL-safe.
    ///
    /// Profile columns are nullable by design ("not measured" is not "measured
    /// zero"), and `max_jaccard >= 0.9` against NULL is NULL rather than
    /// FALSE. An un-coalesced predicate therefore makes `NOT (auto)` NULL and
    /// silently drops every document the policy has not measured with *every*
    /// signal — which the simulation reports as "this policy would do
    /// nothing". Found on live data, where a quality-only scan simulated to
    /// zero while the per-signal breakdown showed 31 hits.
    #[test]
    fn band_predicates_treat_unmeasured_columns_as_no_match_not_as_unknown() {
        for policy in [Policy::conservative(), Policy::standard(), Policy::aggressive()] {
            for band in [Band::Auto, Band::Review] {
                let sql = band_predicate(&policy, band);
                assert!(
                    sql == "FALSE" || sql.starts_with("COALESCE("),
                    "band predicate must be NULL-safe, got: {sql}"
                );
                assert!(
                    sql == "FALSE" || sql.ends_with(", FALSE)"),
                    "band predicate must default to FALSE, got: {sql}"
                );
            }
        }
    }

    #[test]
    fn a_policy_with_everything_off_selects_nothing_rather_than_everything() {
        let policy = Policy {
            exact_duplicate: Band::None,
            url_variant: Band::None,
            asset: Band::None,
            stub: Band::None,
            near_duplicate: Threshold::default(),
            semantic: Threshold::default(),
            quality: QualityThreshold {
                review_min_failures: None,
                min_families: 2,
                auto_min_failures: None,
            },
            off_topic_percentile: None,
            exempt_connectors: Vec::new(),
            cross_connector_review_only: true,
        };
        assert_eq!(band_predicate(&policy, Band::Auto), "FALSE");
        assert_eq!(band_predicate(&policy, Band::Review), "FALSE");
    }

    #[test]
    fn exempt_connector_names_are_escaped_into_the_predicate() {
        let mut policy = Policy::standard();
        policy.exempt_connectors = vec!["o'reilly".into()];
        let sql = exemption_predicate(&policy);
        assert!(sql.contains("'o''reilly'"), "quote must be doubled: {sql}");
    }

    #[test]
    fn exemptions_always_exclude_open_lifecycle_rows_and_exclusions() {
        let sql = exemption_predicate(&Policy::standard());
        assert!(sql.contains("prune_candidate"), "{sql}");
        assert!(sql.contains("prune_exclusions"), "{sql}");
        assert!(sql.contains("public.document"), "{sql}");
    }

    #[test]
    fn policies_round_trip_through_json() {
        for policy in [Policy::conservative(), Policy::standard(), Policy::aggressive()] {
            let json = serde_json::to_value(&policy).unwrap();
            let back: Policy = serde_json::from_value(json).unwrap();
            assert_eq!(policy, back);
        }
    }

    #[test]
    fn an_unknown_policy_key_is_rejected_rather_than_ignored() {
        let err = serde_json::from_value::<Policy>(serde_json::json!({
            "exact_duplicate": "auto",
            "near_dupe": { "auto": 0.9 }
        }))
        .unwrap_err();
        assert!(err.to_string().contains("near_dupe"), "{err}");
    }

    #[test]
    fn histogram_signals_are_all_real_profile_columns() {
        // Guards against a typo turning into a SQL error at request time.
        let ddl = include_str!("prune.rs");
        for signal in HISTOGRAM_SIGNALS {
            assert!(
                ddl.contains(&format!("{signal} ")),
                "{signal} is not a doc_profile column"
            );
        }
    }
}
