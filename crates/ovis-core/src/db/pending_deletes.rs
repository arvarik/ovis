//! The one thing OVIS owns in this database: a retry queue for index deletes
//! that outlived their Postgres transaction.
//!
//! Postgres and OpenSearch have no shared transaction, so a per-document delete
//! commits in Postgres and then removes chunks. If that second step fails, the
//! chunks would orphan forever — the old code acknowledged exactly this in a
//! comment and shipped it. Instead the id lands here and a background task
//! drains it.
//!
//! Everything lives in an `ovis` schema, created on demand. This is the only DDL
//! OVIS ever runs; it never touches Onyx's own tables. If the database user
//! cannot create it, the feature degrades to a loud warning rather than
//! preventing startup.

use sqlx::{PgPool, Row};

use crate::error::CoreResult;
use crate::search::OsClient;

const CREATE_SCHEMA: &str = "CREATE SCHEMA IF NOT EXISTS ovis";

const CREATE_TABLE: &str = "\
CREATE TABLE IF NOT EXISTS ovis.pending_index_deletes ( \
    document_id  text PRIMARY KEY, \
    queued_at    timestamptz NOT NULL DEFAULT now(), \
    attempts     int NOT NULL DEFAULT 0, \
    last_attempt timestamptz, \
    last_error   text \
)";

/// Create the `ovis` schema and queue table. Returns `false` when the database
/// user is not allowed to, in which case a failed index delete is reported to the
/// caller but not retried.
pub async fn ensure_table(pool: &PgPool) -> bool {
    if let Err(err) = sqlx::query(CREATE_SCHEMA).execute(pool).await {
        tracing::warn!(
            error = %err,
            "cannot create the `ovis` schema; failed index deletes will be reported \
             but not retried"
        );
        return false;
    }
    if let Err(err) = sqlx::query(CREATE_TABLE).execute(pool).await {
        tracing::warn!(
            error = %err,
            "cannot create ovis.pending_index_deletes; failed index deletes will be \
             reported but not retried"
        );
        return false;
    }
    true
}

/// Queue a document id whose chunks could not be removed.
pub async fn enqueue(pool: &PgPool, document_id: &str, error: &str) -> CoreResult<()> {
    sqlx::query(
        "INSERT INTO ovis.pending_index_deletes (document_id, last_error) \
         VALUES ($1, $2) \
         ON CONFLICT (document_id) DO UPDATE \
           SET last_error = excluded.last_error, queued_at = now()",
    )
    .bind(document_id)
    .bind(truncate(error, 2000))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn pending_count(pool: &PgPool) -> CoreResult<i64> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ovis.pending_index_deletes")
        .fetch_one(pool)
        .await?;
    Ok(count)
}

/// Outcome of one drain pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub attempted: usize,
    pub cleared: usize,
    pub still_failing: usize,
}

/// Retry queued index deletes. Called on a timer by the backend.
///
/// A document whose chunks are already gone counts as cleared: the goal state is
/// "no chunks for this id", not "we personally deleted them".
pub async fn drain(
    pool: &PgPool,
    os: &OsClient,
    index: &str,
    batch: i64,
) -> CoreResult<DrainReport> {
    let rows = sqlx::query(
        "SELECT document_id FROM ovis.pending_index_deletes \
         ORDER BY queued_at \
         LIMIT $1",
    )
    .bind(batch)
    .fetch_all(pool)
    .await?;

    let mut report = DrainReport::default();
    for row in rows {
        let id: String = row.get("document_id");
        report.attempted += 1;
        match os.delete_document_chunks(index, &id).await {
            Ok(deleted) => {
                sqlx::query("DELETE FROM ovis.pending_index_deletes WHERE document_id = $1")
                    .bind(&id)
                    .execute(pool)
                    .await?;
                report.cleared += 1;
                tracing::info!(
                    document_id = %id,
                    chunks_deleted = deleted,
                    "drained a pending index delete"
                );
            }
            Err(err) => {
                report.still_failing += 1;
                sqlx::query(
                    "UPDATE ovis.pending_index_deletes \
                     SET attempts = attempts + 1, last_attempt = now(), last_error = $2 \
                     WHERE document_id = $1",
                )
                .bind(&id)
                .bind(truncate(&err.to_string(), 2000))
                .execute(pool)
                .await?;
            }
        }
    }
    Ok(report)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_confined_to_the_ovis_schema() {
        // OVIS must never issue DDL against an Onyx table.
        assert!(CREATE_SCHEMA.contains("ovis"));
        assert!(CREATE_TABLE.contains("ovis.pending_index_deletes"));
        assert!(!CREATE_TABLE.contains("public."));
    }

    #[test]
    fn truncate_respects_utf8_boundaries() {
        let s = "aé😀ü";
        for max in 0..s.len() + 2 {
            let out = truncate(s, max);
            assert!(out.len() <= max || max >= s.len());
            assert!(s.starts_with(&out));
        }
        assert_eq!(truncate("short", 100), "short");
    }
}
