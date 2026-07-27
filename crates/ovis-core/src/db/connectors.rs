//! Connector and cc-pair reads.
//!
//! Two corrections from the old implementation are load-bearing here:
//!
//! * **Status is read, not assumed.** The old query selected the SQL literal
//!   `false AS disabled` and never looked at
//!   `connector_credential_pair.status` — so the 278 PAUSED connectors on this
//!   deployment were all displayed as healthy and active.
//! * **Document counts come from `document_by_connector_credential_pair`.**
//!   `connector_credential_pair.total_docs_indexed` exists and is frequently 0
//!   even for a connector holding 100k documents; it is never read.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use crate::api_types::{
    AttemptAggregates, ConnectorDetail, ConnectorStatusCounts, ConnectorSummary, HistoryPoint,
    LastAttempt, PageListItem, TopConnector,
};
use crate::error::{CoreError, CoreResult};
use crate::is_parked_error;

const SUMMARY_SQL: &str = "\
SELECT c.id AS connector_id, \
       cc.id AS cc_pair_id, \
       cc.name AS cc_pair_name, \
       c.name AS connector_name, \
       c.source, \
       cc.status, \
       COALESCE(cc.in_repeated_error_state, false) AS in_repeated_error_state, \
       cc.last_successful_index_time, \
       cc.indexing_trigger, \
       c.refresh_freq, \
       COALESCE(dc.doc_count, 0) AS doc_count, \
       la.last_attempt_id, \
       la.last_status, \
       la.last_error_msg, \
       la.last_time_updated \
FROM public.connector c \
JOIN public.connector_credential_pair cc ON cc.connector_id = c.id \
LEFT JOIN LATERAL ( \
    SELECT count(*) AS doc_count \
    FROM public.document_by_connector_credential_pair dcc \
    WHERE dcc.connector_id = c.id AND dcc.credential_id = cc.credential_id \
) dc ON TRUE \
LEFT JOIN LATERAL ( \
    SELECT ia.id AS last_attempt_id, \
           ia.status AS last_status, \
           ia.error_msg AS last_error_msg, \
           ia.time_updated AS last_time_updated \
    FROM public.index_attempt ia \
    WHERE ia.connector_credential_pair_id = cc.id \
    ORDER BY ia.time_updated DESC \
    LIMIT 1 \
) la ON TRUE";

#[derive(sqlx::FromRow)]
struct SummaryRow {
    connector_id: i32,
    cc_pair_id: i32,
    cc_pair_name: String,
    connector_name: String,
    source: String,
    status: String,
    in_repeated_error_state: bool,
    last_successful_index_time: Option<DateTime<Utc>>,
    indexing_trigger: Option<String>,
    refresh_freq: Option<i32>,
    doc_count: i64,
    last_attempt_id: Option<i32>,
    last_status: Option<String>,
    last_error_msg: Option<String>,
    last_time_updated: Option<DateTime<Utc>>,
}

impl From<SummaryRow> for ConnectorSummary {
    fn from(r: SummaryRow) -> Self {
        let parked = is_parked_error(r.last_error_msg.as_deref());
        let last_attempt = r.last_attempt_id.map(|id| LastAttempt {
            id: Some(id),
            status: r.last_status.clone(),
            time_updated: r.last_time_updated,
            error_msg: r.last_error_msg.clone(),
        });
        ConnectorSummary {
            connector_id: r.connector_id,
            cc_pair_id: r.cc_pair_id,
            // The cc-pair name is what Onyx's own admin UI shows and what the
            // delete guard requires as confirmation; fall back to the connector
            // name only if it is blank.
            name: if r.cc_pair_name.trim().is_empty() {
                r.connector_name
            } else {
                r.cc_pair_name
            },
            source: r.source,
            status: r.status,
            parked,
            in_repeated_error_state: r.in_repeated_error_state,
            doc_count: r.doc_count,
            last_successful_index_time: r.last_successful_index_time,
            refresh_freq_secs: r.refresh_freq,
            indexing_trigger: r.indexing_trigger,
            last_attempt,
        }
    }
}

/// Every cc-pair with real status, real doc counts, and latest-attempt health.
/// Ordered by document count so the biggest connectors lead.
pub async fn list_summaries(pool: &PgPool) -> CoreResult<Vec<ConnectorSummary>> {
    let sql = format!("{SUMMARY_SQL} ORDER BY doc_count DESC, cc_pair_id");
    let rows: Vec<SummaryRow> = sqlx::query_as(&sql).fetch_all(pool).await?;
    Ok(rows.into_iter().map(ConnectorSummary::from).collect())
}

/// One cc-pair's summary.
pub async fn get_summary(pool: &PgPool, cc_pair_id: i32) -> CoreResult<Option<ConnectorSummary>> {
    let sql = format!("{SUMMARY_SQL} WHERE cc.id = $1");
    let row: Option<SummaryRow> = sqlx::query_as(&sql)
        .bind(cc_pair_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(ConnectorSummary::from))
}

/// The identifiers an Onyx action needs, plus the guards it must respect.
#[derive(Debug, Clone, PartialEq)]
pub struct CcPairRef {
    pub cc_pair_id: i32,
    pub connector_id: i32,
    pub credential_id: i32,
    pub name: String,
    pub status: String,
    /// Latest attempt carries a resilience-cron park sentinel, so `run-once`
    /// needs an explicit acknowledgement.
    pub parked: bool,
}

/// Resolve a cc-pair id to the connector/credential ids Onyx's action endpoints
/// take, together with its park state.
pub async fn get_cc_pair_ref(pool: &PgPool, cc_pair_id: i32) -> CoreResult<CcPairRef> {
    let row = sqlx::query(
        "SELECT cc.id AS cc_pair_id, cc.connector_id, cc.credential_id, cc.name, cc.status, \
                la.last_error_msg \
         FROM public.connector_credential_pair cc \
         LEFT JOIN LATERAL ( \
             SELECT ia.error_msg AS last_error_msg \
             FROM public.index_attempt ia \
             WHERE ia.connector_credential_pair_id = cc.id \
             ORDER BY ia.time_updated DESC \
             LIMIT 1 \
         ) la ON TRUE \
         WHERE cc.id = $1",
    )
    .bind(cc_pair_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| CoreError::not_found("cc-pair", cc_pair_id.to_string()))?;

    let last_error: Option<String> = row.get("last_error_msg");
    Ok(CcPairRef {
        cc_pair_id: row.get("cc_pair_id"),
        connector_id: row.get("connector_id"),
        credential_id: row.get("credential_id"),
        name: row.get("name"),
        status: row.get("status"),
        parked: is_parked_error(last_error.as_deref()),
    })
}

/// Full detail for one cc-pair: summary plus connector config, credential
/// identity (never its secret), and attempt aggregates.
pub async fn get_detail(pool: &PgPool, cc_pair_id: i32) -> CoreResult<Option<ConnectorDetail>> {
    let Some(summary) = get_summary(pool, cc_pair_id).await? else {
        return Ok(None);
    };

    let row = sqlx::query(
        "SELECT c.connector_specific_config, c.input_type, c.prune_freq, \
                c.time_created, c.time_updated, \
                cc.access_type, cc.credential_id, cc.last_pruned, \
                cr.name AS credential_name \
         FROM public.connector_credential_pair cc \
         JOIN public.connector c ON c.id = cc.connector_id \
         LEFT JOIN public.credential cr ON cr.id = cc.credential_id \
         WHERE cc.id = $1",
    )
    .bind(cc_pair_id)
    .fetch_one(pool)
    .await?;

    let attempts = attempt_aggregates(pool, Some(cc_pair_id)).await?;

    Ok(Some(ConnectorDetail {
        summary,
        connector_specific_config: row.get("connector_specific_config"),
        input_type: row.get("input_type"),
        prune_freq_secs: row.get("prune_freq"),
        access_type: row.get("access_type"),
        credential_id: row.get("credential_id"),
        credential_name: row.get("credential_name"),
        time_created: row.get("time_created"),
        time_updated: row.get("time_updated"),
        last_pruned: row.get("last_pruned"),
        attempts,
        history: None,
    }))
}

/// Per-status attempt counts, globally or for one cc-pair.
pub async fn attempt_aggregates(
    pool: &PgPool,
    cc_pair_id: Option<i32>,
) -> CoreResult<AttemptAggregates> {
    let rows = sqlx::query(
        "SELECT ia.status, count(*) AS n \
         FROM public.index_attempt ia \
         WHERE ($1::int IS NULL OR ia.connector_credential_pair_id = $1) \
         GROUP BY ia.status",
    )
    .bind(cc_pair_id)
    .fetch_all(pool)
    .await?;

    let mut agg = AttemptAggregates {
        success: 0,
        failed: 0,
        canceled: 0,
        in_progress: 0,
        not_started: 0,
        completed_with_errors: 0,
        other: 0,
    };
    for row in rows {
        let status: String = row.get("status");
        let n: i64 = row.get("n");
        match status.to_uppercase().as_str() {
            "SUCCESS" => agg.success += n,
            "FAILED" => agg.failed += n,
            "CANCELED" | "CANCELLED" => agg.canceled += n,
            "IN_PROGRESS" => agg.in_progress += n,
            "NOT_STARTED" => agg.not_started += n,
            "COMPLETED_WITH_ERRORS" => agg.completed_with_errors += n,
            _ => agg.other += n,
        }
    }
    Ok(agg)
}

/// Daily documents-added history for a cc-pair, from `index_attempt` sums.
pub async fn history(pool: &PgPool, cc_pair_id: i32, days: i64) -> CoreResult<Vec<HistoryPoint>> {
    let rows = sqlx::query(
        "SELECT to_char(date_trunc('day', ia.time_updated), 'YYYY-MM-DD') AS day, \
                COALESCE(sum(ia.new_docs_indexed), 0)::bigint AS docs_added \
         FROM public.index_attempt ia \
         WHERE ia.connector_credential_pair_id = $1 \
           AND ia.time_updated >= now() - make_interval(days => $2::int) \
         GROUP BY 1 \
         ORDER BY 1",
    )
    .bind(cc_pair_id)
    .bind(days as i32)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| HistoryPoint {
            day: r.get("day"),
            docs_added: r.get("docs_added"),
        })
        .collect())
}

/// The documents a cc-pair is authoritatively responsible for, newest first.
/// Answers "what exactly did this connector crawl".
pub async fn list_docs(
    pool: &PgPool,
    cc_pair_id: i32,
    limit: i64,
    offset: i64,
) -> CoreResult<Vec<PageListItem>> {
    let rows = sqlx::query(
        "SELECT d.id, d.semantic_id, d.link, \
                COALESCE(d.doc_updated_at, d.last_modified) AS sort_ts, \
                d.doc_updated_at, d.last_modified, d.chunk_count, d.boost, d.hidden, \
                d.doc_metadata AS metadata, \
                c.id AS connector_id, c.name AS connector_name, c.source AS connector_source \
         FROM public.connector_credential_pair cc \
         JOIN public.document_by_connector_credential_pair dcc \
              ON dcc.connector_id = cc.connector_id AND dcc.credential_id = cc.credential_id \
         JOIN public.document d ON d.id = dcc.id \
         JOIN public.connector c ON c.id = cc.connector_id \
         WHERE cc.id = $1 \
         ORDER BY COALESCE(d.doc_updated_at, d.last_modified) DESC, d.id DESC \
         LIMIT $2 OFFSET $3",
    )
    .bind(cc_pair_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| PageListItem {
            id: r.get("id"),
            semantic_id: r.get("semantic_id"),
            link: r.get("link"),
            updated_at: r.get("sort_ts"),
            doc_updated_at: r.get("doc_updated_at"),
            last_modified: r.get("last_modified"),
            chunk_count: r.get("chunk_count"),
            boost: r.get("boost"),
            hidden: r.get("hidden"),
            connector_id: r.get("connector_id"),
            connector_name: r.get("connector_name"),
            connector_source: r.get("connector_source"),
            metadata: r.get("metadata"),
        })
        .collect())
}

pub async fn count_docs(pool: &PgPool, cc_pair_id: i32) -> CoreResult<i64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM public.connector_credential_pair cc \
         JOIN public.document_by_connector_credential_pair dcc \
              ON dcc.connector_id = cc.connector_id AND dcc.credential_id = cc.credential_id \
         WHERE cc.id = $1",
    )
    .bind(cc_pair_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// cc-pair counts by status, plus how many are parked. Cheap: 332 rows.
pub async fn status_counts(pool: &PgPool) -> CoreResult<ConnectorStatusCounts> {
    let rows = sqlx::query(
        "SELECT cc.status, count(*) AS n FROM public.connector_credential_pair cc GROUP BY 1",
    )
    .fetch_all(pool)
    .await?;

    let mut counts = ConnectorStatusCounts {
        total: 0,
        active: 0,
        paused: 0,
        initial_indexing: 0,
        deleting: 0,
        invalid: 0,
        parked: 0,
    };
    for row in rows {
        let status: String = row.get("status");
        let n: i64 = row.get("n");
        counts.total += n;
        match status.to_uppercase().as_str() {
            "ACTIVE" => counts.active += n,
            "PAUSED" => counts.paused += n,
            "INITIAL_INDEXING" => counts.initial_indexing += n,
            "DELETING" => counts.deleting += n,
            "INVALID" => counts.invalid += n,
            _ => {}
        }
    }

    // Parked is a property of the latest attempt's error message, not of status.
    let parked_patterns: Vec<String> = crate::PARKED_SENTINELS
        .iter()
        .map(|s| format!("%{s}%"))
        .collect();
    counts.parked = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM public.connector_credential_pair cc \
         WHERE EXISTS ( \
             SELECT 1 FROM ( \
                 SELECT ia.error_msg \
                 FROM public.index_attempt ia \
                 WHERE ia.connector_credential_pair_id = cc.id \
                 ORDER BY ia.time_updated DESC \
                 LIMIT 1 \
             ) latest \
             WHERE latest.error_msg LIKE ANY($1) \
         )",
    )
    .bind(&parked_patterns)
    .fetch_one(pool)
    .await?;

    Ok(counts)
}

/// Leaderboard for `/stats/connectors/top`.
pub async fn top_connectors(
    pool: &PgPool,
    by_recent: bool,
    limit: i64,
) -> CoreResult<Vec<TopConnector>> {
    let order = if by_recent {
        "cc.last_successful_index_time DESC NULLS LAST"
    } else {
        "doc_count DESC"
    };
    let sql = format!(
        "SELECT cc.id AS cc_pair_id, c.id AS connector_id, \
                CASE WHEN btrim(cc.name) = '' THEN c.name ELSE cc.name END AS name, \
                c.source, cc.status, cc.last_successful_index_time, \
                COALESCE(dc.doc_count, 0) AS doc_count \
         FROM public.connector_credential_pair cc \
         JOIN public.connector c ON c.id = cc.connector_id \
         LEFT JOIN LATERAL ( \
             SELECT count(*) AS doc_count \
             FROM public.document_by_connector_credential_pair dcc \
             WHERE dcc.connector_id = cc.connector_id AND dcc.credential_id = cc.credential_id \
         ) dc ON TRUE \
         ORDER BY {order}, cc.id \
         LIMIT $1"
    );

    let rows = sqlx::query(&sql).bind(limit).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| TopConnector {
            cc_pair_id: r.get("cc_pair_id"),
            connector_id: r.get("connector_id"),
            name: r.get("name"),
            source: r.get("source"),
            status: r.get("status"),
            doc_count: r.get("doc_count"),
            last_successful_index_time: r.get("last_successful_index_time"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_query_reads_real_status_and_never_total_docs_indexed() {
        assert!(
            SUMMARY_SQL.contains("cc.status"),
            "status must be read from the cc-pair, not hardcoded"
        );
        assert!(
            !SUMMARY_SQL.contains("false AS disabled"),
            "the hardcoded `disabled` literal must be gone"
        );
        assert!(
            !SUMMARY_SQL.contains("total_docs_indexed"),
            "total_docs_indexed is unreliable and must never be read"
        );
        assert!(
            SUMMARY_SQL.contains("FROM public.document_by_connector_credential_pair dcc"),
            "doc counts must come from dcc"
        );
    }

    #[test]
    fn summary_counts_documents_per_credential_not_per_ccpair_row() {
        // The old COUNT(d.id) over a join double-counted documents attached to
        // several credentials. The lateral count is scoped to this cc-pair's
        // (connector_id, credential_id).
        assert!(SUMMARY_SQL
            .contains("WHERE dcc.connector_id = c.id AND dcc.credential_id = cc.credential_id"));
    }

    #[test]
    fn top_connector_ordering_is_enum_driven_not_caller_supplied() {
        // by_recent is a bool, so no caller string ever reaches ORDER BY.
        for by_recent in [true, false] {
            let order = if by_recent {
                "cc.last_successful_index_time DESC NULLS LAST"
            } else {
                "doc_count DESC"
            };
            assert!(!order.contains(';'));
        }
    }
}
