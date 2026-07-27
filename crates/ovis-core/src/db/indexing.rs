//! Indexing telemetry: read-only views over `index_attempt`,
//! `index_attempt_errors` and `background_error`.
//!
//! 8.3k attempt rows carry the entire crawl history of this deployment and the
//! old API exposed none of it.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use crate::api_types::{BackgroundErrorItem, IndexAttemptError, IndexAttemptItem};
use crate::error::CoreResult;
use crate::is_parked_error;

/// An `IN_PROGRESS` attempt whose `time_updated` is older than this is treated as
/// stalled — the same heuristic the homelab resilience cron uses.
///
/// Staleness, never doc counts: a healthy connector can legitimately sit at zero
/// new documents for a long time, so counting documents would flag working
/// crawls as dead.
pub const STALL_AFTER_MINUTES: i64 = 45;

/// Rolling retention of `index_attempt_errors`, enforced by the resilience cron.
/// Reported in the response so an empty list is never read as "never failed".
pub const ATTEMPT_ERROR_WINDOW: &str = "24h";

const ATTEMPT_SELECT: &str = "\
SELECT ia.id, \
       ia.connector_credential_pair_id AS cc_pair_id, \
       cc.connector_id, \
       c.name AS connector_name, \
       ia.status, \
       ia.new_docs_indexed, \
       ia.total_docs_indexed, \
       ia.docs_removed_from_index, \
       ia.total_chunks, \
       ia.completed_batches, \
       ia.total_batches, \
       ia.total_failures_batch_level, \
       ia.time_created, \
       ia.time_started, \
       ia.time_updated, \
       ia.error_msg, \
       ia.from_beginning, \
       ia.poll_range_start, \
       ia.poll_range_end, \
       ia.last_heartbeat_time, \
       ia.heartbeat_counter, \
       ia.cancellation_requested, \
       ia.search_settings_id \
FROM public.index_attempt ia \
LEFT JOIN public.connector_credential_pair cc ON cc.id = ia.connector_credential_pair_id \
LEFT JOIN public.connector c ON c.id = cc.connector_id";

#[derive(sqlx::FromRow)]
struct AttemptRow {
    id: i32,
    cc_pair_id: i32,
    connector_id: Option<i32>,
    connector_name: Option<String>,
    status: String,
    new_docs_indexed: Option<i32>,
    total_docs_indexed: Option<i32>,
    docs_removed_from_index: Option<i32>,
    total_chunks: i32,
    completed_batches: i32,
    total_batches: Option<i32>,
    total_failures_batch_level: i32,
    time_created: DateTime<Utc>,
    time_started: Option<DateTime<Utc>>,
    time_updated: DateTime<Utc>,
    error_msg: Option<String>,
    from_beginning: bool,
    poll_range_start: Option<DateTime<Utc>>,
    poll_range_end: Option<DateTime<Utc>>,
    last_heartbeat_time: Option<DateTime<Utc>>,
    heartbeat_counter: i32,
    cancellation_requested: bool,
    search_settings_id: Option<i32>,
}

impl AttemptRow {
    fn into_item(self, now: DateTime<Utc>) -> IndexAttemptItem {
        let running = self.status.eq_ignore_ascii_case("IN_PROGRESS");

        // Heartbeat if we have one, otherwise the row's own update time.
        let liveness = self.last_heartbeat_time.unwrap_or(self.time_updated);
        let stalled = running && (now - liveness) > chrono::Duration::minutes(STALL_AFTER_MINUTES);

        let pages_per_min = if running {
            self.time_started.and_then(|started| {
                let mins = (now - started).num_milliseconds() as f64 / 60_000.0;
                let docs = self.total_docs_indexed.or(self.new_docs_indexed)? as f64;
                // Below a few seconds the rate is noise, not information.
                (mins > 0.05).then(|| (docs / mins * 100.0).round() / 100.0)
            })
        } else {
            None
        };

        IndexAttemptItem {
            parked: is_parked_error(self.error_msg.as_deref()),
            stalled,
            pages_per_min,
            id: self.id,
            cc_pair_id: self.cc_pair_id,
            connector_id: self.connector_id,
            connector_name: self.connector_name,
            status: self.status,
            new_docs_indexed: self.new_docs_indexed,
            total_docs_indexed: self.total_docs_indexed,
            docs_removed_from_index: self.docs_removed_from_index,
            total_chunks: self.total_chunks,
            completed_batches: self.completed_batches,
            total_batches: self.total_batches,
            total_failures_batch_level: self.total_failures_batch_level,
            time_created: self.time_created,
            time_started: self.time_started,
            time_updated: self.time_updated,
            error_msg: self.error_msg,
            from_beginning: self.from_beginning,
            poll_range_start: self.poll_range_start,
            poll_range_end: self.poll_range_end,
            last_heartbeat_time: self.last_heartbeat_time,
            heartbeat_counter: self.heartbeat_counter,
            cancellation_requested: self.cancellation_requested,
            search_settings_id: self.search_settings_id,
        }
    }
}

/// Attempt history, newest first. `cc_pair_id` scopes it to one connector;
/// `statuses` filters (case-insensitively, upper-cased by the caller).
pub async fn list_attempts(
    pool: &PgPool,
    cc_pair_id: Option<i32>,
    statuses: Option<&[String]>,
    limit: i64,
    offset: i64,
) -> CoreResult<Vec<IndexAttemptItem>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(ATTEMPT_SELECT);
    qb.push(" WHERE TRUE");
    if let Some(id) = cc_pair_id {
        qb.push(" AND ia.connector_credential_pair_id = ");
        qb.push_bind(id);
    }
    if let Some(statuses) = statuses.filter(|s| !s.is_empty()) {
        qb.push(" AND upper(ia.status) = ANY(");
        qb.push_bind(
            statuses
                .iter()
                .map(|s| s.to_uppercase())
                .collect::<Vec<String>>(),
        );
        qb.push(")");
    }
    qb.push(" ORDER BY ia.time_updated DESC, ia.id DESC LIMIT ");
    qb.push_bind(limit);
    qb.push(" OFFSET ");
    qb.push_bind(offset);

    let rows: Vec<AttemptRow> = qb.build_query_as().fetch_all(pool).await?;
    let now = Utc::now();
    Ok(rows.into_iter().map(|r| r.into_item(now)).collect())
}

pub async fn count_attempts(
    pool: &PgPool,
    cc_pair_id: Option<i32>,
    statuses: Option<&[String]>,
) -> CoreResult<i64> {
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT count(*) FROM public.index_attempt ia WHERE TRUE");
    if let Some(id) = cc_pair_id {
        qb.push(" AND ia.connector_credential_pair_id = ");
        qb.push_bind(id);
    }
    if let Some(statuses) = statuses.filter(|s| !s.is_empty()) {
        qb.push(" AND upper(ia.status) = ANY(");
        qb.push_bind(
            statuses
                .iter()
                .map(|s| s.to_uppercase())
                .collect::<Vec<String>>(),
        );
        qb.push(")");
    }
    let count: i64 = qb.build_query_scalar().fetch_one(pool).await?;
    Ok(count)
}

pub async fn get_attempt(pool: &PgPool, id: i32) -> CoreResult<Option<IndexAttemptItem>> {
    let sql = format!("{ATTEMPT_SELECT} WHERE ia.id = $1");
    let row: Option<AttemptRow> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    Ok(row.map(|r| r.into_item(Utc::now())))
}

/// Count of attempts that are `IN_PROGRESS`, and of those, how many are stalled.
pub async fn in_progress_counts(pool: &PgPool) -> CoreResult<(i64, i64)> {
    let row = sqlx::query(
        "SELECT count(*) AS running, \
                count(*) FILTER ( \
                    WHERE COALESCE(ia.last_heartbeat_time, ia.time_updated) \
                          < now() - make_interval(mins => $1::int) \
                ) AS stalled \
         FROM public.index_attempt ia \
         WHERE upper(ia.status) = 'IN_PROGRESS'",
    )
    .bind(STALL_AFTER_MINUTES as i32)
    .fetch_one(pool)
    .await?;
    Ok((row.get("running"), row.get("stalled")))
}

/// Per-document indexing failures for a cc-pair. Rolling 24 h window.
pub async fn list_attempt_errors(
    pool: &PgPool,
    cc_pair_id: Option<i32>,
    unresolved_only: bool,
    limit: i64,
    offset: i64,
) -> CoreResult<Vec<IndexAttemptError>> {
    let rows = sqlx::query(
        "SELECT e.id, e.index_attempt_id, e.connector_credential_pair_id AS cc_pair_id, \
                e.document_id, e.document_link, e.failure_message, e.error_type, \
                e.time_created, e.is_resolved \
         FROM public.index_attempt_errors e \
         WHERE ($1::int IS NULL OR e.connector_credential_pair_id = $1) \
           AND ($2::bool IS FALSE OR e.is_resolved = FALSE) \
         ORDER BY e.time_created DESC, e.id DESC \
         LIMIT $3 OFFSET $4",
    )
    .bind(cc_pair_id)
    .bind(unresolved_only)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| IndexAttemptError {
            id: r.get("id"),
            index_attempt_id: r.get("index_attempt_id"),
            cc_pair_id: r.get("cc_pair_id"),
            document_id: r.get("document_id"),
            document_link: r.get("document_link"),
            failure_message: r.get("failure_message"),
            error_type: r.get("error_type"),
            time_created: r.get("time_created"),
            is_resolved: r.get("is_resolved"),
        })
        .collect())
}

pub async fn count_attempt_errors(
    pool: &PgPool,
    cc_pair_id: Option<i32>,
    unresolved_only: bool,
) -> CoreResult<i64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.index_attempt_errors e \
         WHERE ($1::int IS NULL OR e.connector_credential_pair_id = $1) \
           AND ($2::bool IS FALSE OR e.is_resolved = FALSE)",
    )
    .bind(cc_pair_id)
    .bind(unresolved_only)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Worker-level failures that are not tied to a single document.
pub async fn list_background_errors(
    pool: &PgPool,
    cc_pair_id: Option<i32>,
    limit: i64,
) -> CoreResult<Vec<BackgroundErrorItem>> {
    let rows = sqlx::query(
        "SELECT id, message, time_created, cc_pair_id \
         FROM public.background_error \
         WHERE ($1::int IS NULL OR cc_pair_id = $1) \
         ORDER BY time_created DESC, id DESC \
         LIMIT $2",
    )
    .bind(cc_pair_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| BackgroundErrorItem {
            id: r.get("id"),
            message: r.get("message"),
            time_created: r.get("time_created"),
            cc_pair_id: r.get("cc_pair_id"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, updated_mins_ago: i64, heartbeat_mins_ago: Option<i64>) -> AttemptRow {
        let now = Utc::now();
        AttemptRow {
            id: 1,
            cc_pair_id: 7,
            connector_id: Some(7),
            connector_name: Some("c".into()),
            status: status.into(),
            new_docs_indexed: Some(120),
            total_docs_indexed: Some(240),
            docs_removed_from_index: None,
            total_chunks: 10,
            completed_batches: 2,
            total_batches: Some(4),
            total_failures_batch_level: 0,
            time_created: now - chrono::Duration::hours(2),
            time_started: Some(now - chrono::Duration::minutes(60)),
            time_updated: now - chrono::Duration::minutes(updated_mins_ago),
            error_msg: None,
            from_beginning: false,
            poll_range_start: None,
            poll_range_end: None,
            last_heartbeat_time: heartbeat_mins_ago.map(|m| now - chrono::Duration::minutes(m)),
            heartbeat_counter: 3,
            cancellation_requested: false,
            search_settings_id: Some(4),
        }
    }

    #[test]
    fn stall_detection_uses_heartbeat_staleness_not_doc_counts() {
        let now = Utc::now();

        // Running, fresh heartbeat, zero new docs — must NOT be flagged.
        let mut fresh = row("IN_PROGRESS", 120, Some(1));
        fresh.new_docs_indexed = Some(0);
        fresh.total_docs_indexed = Some(0);
        assert!(!fresh.into_item(now).stalled);

        // Running, no heartbeat at all, row untouched for an hour — stalled.
        assert!(row("IN_PROGRESS", 60, None).into_item(now).stalled);

        // Heartbeat older than the threshold — stalled.
        assert!(row("IN_PROGRESS", 1, Some(90)).into_item(now).stalled);

        // Not running: never stalled, however old.
        assert!(!row("SUCCESS", 10_000, None).into_item(now).stalled);
        assert!(!row("FAILED", 10_000, None).into_item(now).stalled);
    }

    #[test]
    fn rate_is_reported_only_for_running_attempts() {
        let now = Utc::now();
        let running = row("IN_PROGRESS", 1, Some(1)).into_item(now);
        // 240 docs over ~60 minutes.
        let rate = running
            .pages_per_min
            .expect("running attempts report a rate");
        assert!((rate - 4.0).abs() < 0.5, "unexpected rate {rate}");

        assert!(row("SUCCESS", 1, None)
            .into_item(now)
            .pages_per_min
            .is_none());
    }

    #[test]
    fn rate_is_suppressed_for_an_attempt_that_just_started() {
        let now = Utc::now();
        let mut just_started = row("IN_PROGRESS", 0, Some(0));
        just_started.time_started = Some(now);
        assert!(
            just_started.into_item(now).pages_per_min.is_none(),
            "a sub-second sample would produce a meaningless rate"
        );
    }

    #[test]
    fn park_sentinel_is_surfaced_and_never_rewritten() {
        let now = Utc::now();
        let mut parked = row("FAILED", 5, None);
        parked.error_msg = Some("park done".into());
        let item = parked.into_item(now);
        assert!(item.parked);
        assert_eq!(
            item.error_msg.as_deref(),
            Some("park done"),
            "the sentinel must be passed through verbatim"
        );
    }

    #[test]
    fn attempt_status_filter_is_bound_and_upper_cased() {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(ATTEMPT_SELECT);
        qb.push(" WHERE TRUE");
        let statuses = ["success'; DROP TABLE index_attempt; --".to_string()];
        qb.push(" AND upper(ia.status) = ANY(");
        qb.push_bind(
            statuses
                .iter()
                .map(|s| s.to_uppercase())
                .collect::<Vec<String>>(),
        );
        qb.push(")");
        let sql = qb.into_sql();
        assert!(!sql.contains("DROP TABLE"));
        assert!(sql.contains("upper(ia.status) = ANY($1)"));
    }
}
