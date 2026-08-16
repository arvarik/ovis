//! Generated descriptions of groups of documents.
//!
//! An annotation is a *sentence about* a cluster or a detector bundle. It never
//! participates in a decision: no query reads it, no policy consults it, and
//! nothing here can move a document. It exists so a reviewer can read
//! "Archived Stanford Encyclopedia editions of entries that are still live"
//! instead of `hash:4f2a91…`.
//!
//! **Generations, not updates.** The primary key includes `(model,
//! prompt_hash)`, so re-narrating with a different model or a changed prompt
//! writes a *new row* beside the old one rather than overwriting it. The read
//! path takes the newest. This is the same versioning rule
//! `prune_minhash.config_hash` applies to signatures and `Grade` applies to
//! scores, and it exists for the same reason: a changed prompt makes old output
//! incomparable, and silently mixing generations is how a prompt edit becomes
//! indistinguishable from a model upgrade.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

use crate::error::{CoreError, CoreResult};

/// What an annotation is attached to.
///
/// Not an enum in the database: the set grows, and a check constraint on a text
/// column would need a migration to add `rule_suggestion` later.
pub const SUBJECT_KINDS: [&str; 2] = ["cluster", "bundle"];

const DDL: &[&str] = &[
    "CREATE SCHEMA IF NOT EXISTS ovis",
    "CREATE TABLE IF NOT EXISTS ovis.llm_annotation ( \
        subject_kind text NOT NULL, \
        subject_key  text NOT NULL, \
        title        text, \
        summary      text, \
        payload      jsonb, \
        model        text NOT NULL, \
        prompt_hash  text NOT NULL, \
        generated_at timestamptz NOT NULL DEFAULT now(), \
        PRIMARY KEY (subject_kind, subject_key, model, prompt_hash) \
    )",
    // The read path is always "newest annotation for these subjects", so the
    // index carries the ordering rather than leaving it to a sort.
    "CREATE INDEX IF NOT EXISTS llm_annotation_newest \
        ON ovis.llm_annotation (subject_kind, subject_key, generated_at DESC)",
];

pub async fn ensure_tables(pool: &PgPool) -> bool {
    for statement in DDL {
        if let Err(err) = sqlx::query(*statement).execute(pool).await {
            tracing::warn!(
                error = %err,
                "cannot create ovis.llm_annotation; generated titles will be unavailable"
            );
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Annotation {
    pub subject_kind: String,
    pub subject_key: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub model: String,
    pub prompt_hash: String,
    pub generated_at: DateTime<Utc>,
}

fn row_to_annotation(r: &sqlx::postgres::PgRow) -> Annotation {
    Annotation {
        subject_kind: r.get("subject_kind"),
        subject_key: r.get("subject_key"),
        title: r.get("title"),
        summary: r.get("summary"),
        model: r.get("model"),
        prompt_hash: r.get("prompt_hash"),
        generated_at: r.get("generated_at"),
    }
}

/// Record one generation.
///
/// `ON CONFLICT DO UPDATE` covers only re-running the *same* prompt on the
/// *same* model — a retry — which should refresh the timestamp rather than
/// fail. A different model or prompt lands on a different key and becomes a new
/// generation, which is the point.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    pool: &PgPool,
    subject_kind: &str,
    subject_key: &str,
    title: &str,
    summary: &str,
    payload: Option<&serde_json::Value>,
    model: &str,
    prompt_hash: &str,
) -> CoreResult<Annotation> {
    if !SUBJECT_KINDS.contains(&subject_kind) {
        return Err(CoreError::Invalid(format!(
            "unknown annotation subject kind '{subject_kind}'; expected one of {}",
            SUBJECT_KINDS.join(", ")
        )));
    }
    let row = sqlx::query(
        "INSERT INTO ovis.llm_annotation \
             (subject_kind, subject_key, title, summary, payload, model, prompt_hash) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (subject_kind, subject_key, model, prompt_hash) DO UPDATE \
             SET title = EXCLUDED.title, \
                 summary = EXCLUDED.summary, \
                 payload = EXCLUDED.payload, \
                 generated_at = now() \
         RETURNING *",
    )
    .bind(subject_kind)
    .bind(subject_key)
    .bind(title)
    .bind(summary)
    .bind(payload)
    .bind(model)
    .bind(prompt_hash)
    .fetch_one(pool)
    .await
    .map_err(CoreError::Db)?;
    Ok(row_to_annotation(&row))
}

/// The newest annotation for each of `keys`, as `(subject_key, annotation)`.
///
/// Returns only what exists. A subject with no annotation is absent rather than
/// present-and-empty, so a caller cannot mistake "not narrated yet" for
/// "narrated, and the model had nothing to say".
pub async fn newest_for(
    pool: &PgPool,
    subject_kind: &str,
    keys: &[String],
) -> CoreResult<Vec<Annotation>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT DISTINCT ON (subject_key) * \
         FROM ovis.llm_annotation \
         WHERE subject_kind = $1 AND subject_key = ANY($2) \
         ORDER BY subject_key, generated_at DESC",
    )
    .bind(subject_kind)
    .bind(keys)
    .fetch_all(pool)
    .await
    .map_err(CoreError::Db)?;
    Ok(rows.iter().map(row_to_annotation).collect())
}

/// Subject keys from `keys` that have no annotation from this exact
/// `(model, prompt_hash)` — the work list for a narration run.
///
/// Keyed on the exact generation rather than on "has any annotation" so that
/// changing the prompt re-narrates everything, which is the behaviour that
/// makes a prompt edit safe to make.
pub async fn missing_generation(
    pool: &PgPool,
    subject_kind: &str,
    keys: &[String],
    model: &str,
    prompt_hash: &str,
) -> CoreResult<Vec<String>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT k FROM unnest($1::text[]) AS k \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM ovis.llm_annotation a \
             WHERE a.subject_kind = $2 AND a.subject_key = k \
               AND a.model = $3 AND a.prompt_hash = $4)",
    )
    .bind(keys)
    .bind(subject_kind)
    .bind(model)
    .bind(prompt_hash)
    .fetch_all(pool)
    .await
    .map_err(CoreError::Db)?;
    Ok(rows.iter().map(|r| r.get("k")).collect())
}

/// How many annotations exist, by subject kind — for the UI's "narrated N of M".
pub async fn counts(pool: &PgPool) -> CoreResult<Vec<(String, i64)>> {
    let rows = sqlx::query(
        "SELECT subject_kind, count(DISTINCT subject_key)::bigint AS n \
         FROM ovis.llm_annotation GROUP BY subject_kind ORDER BY subject_kind",
    )
    .fetch_all(pool)
    .await
    .map_err(CoreError::Db)?;
    Ok(rows
        .iter()
        .map(|r| (r.get("subject_kind"), r.get("n")))
        .collect())
}
