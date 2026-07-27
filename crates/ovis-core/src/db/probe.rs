//! Startup (and periodic) verification that the Onyx schema still looks the way
//! this code assumes.
//!
//! Onyx upgrades move columns. Without this, a renamed column turns into either
//! a 500 on a hot path or — worse — a query that still runs and quietly answers
//! the wrong question. With it, the mismatch is named in `/system/health` and the
//! endpoints that depend on the missing column return `501 SCHEMA_MISMATCH`.
//!
//! This module also resolves the *real* OpenSearch index name from
//! `search_settings`. The old code hardcoded the wildcard `danswer_chunk*`,
//! which would fan a delete or update out across a secondary index during an
//! Onyx re-embed. Four `search_settings` rows exist on this deployment; exactly
//! one is `PRESENT`.

use std::collections::BTreeSet;

use sqlx::{PgPool, Row};

use crate::error::{CoreError, CoreResult};

use super::documents::DOCUMENT_FK_CHILD_TABLES;

/// Every `table.column` the data layer reads. Kept as one flat list so the probe
/// is a single query and adding a column to a query is one line here.
pub const REQUIRED_COLUMNS: &[(&str, &str)] = &[
    ("document", "id"),
    ("document", "semantic_id"),
    ("document", "link"),
    ("document", "doc_updated_at"),
    ("document", "last_modified"),
    ("document", "last_synced"),
    ("document", "chunk_count"),
    ("document", "boost"),
    ("document", "hidden"),
    ("document", "doc_metadata"),
    ("document", "primary_owners"),
    ("document", "secondary_owners"),
    ("document", "content_hash"),
    ("document", "from_ingestion_api"),
    ("document_by_connector_credential_pair", "id"),
    ("document_by_connector_credential_pair", "connector_id"),
    ("document_by_connector_credential_pair", "credential_id"),
    ("document_by_connector_credential_pair", "has_been_indexed"),
    ("connector", "id"),
    ("connector", "name"),
    ("connector", "source"),
    ("connector", "input_type"),
    ("connector", "connector_specific_config"),
    ("connector", "refresh_freq"),
    ("connector", "prune_freq"),
    ("connector", "time_created"),
    ("connector", "time_updated"),
    ("connector_credential_pair", "id"),
    ("connector_credential_pair", "name"),
    ("connector_credential_pair", "connector_id"),
    ("connector_credential_pair", "credential_id"),
    ("connector_credential_pair", "status"),
    ("connector_credential_pair", "access_type"),
    ("connector_credential_pair", "indexing_trigger"),
    ("connector_credential_pair", "in_repeated_error_state"),
    ("connector_credential_pair", "last_successful_index_time"),
    ("connector_credential_pair", "last_pruned"),
    ("credential", "id"),
    ("credential", "name"),
    ("index_attempt", "id"),
    ("index_attempt", "connector_credential_pair_id"),
    ("index_attempt", "search_settings_id"),
    ("index_attempt", "status"),
    ("index_attempt", "new_docs_indexed"),
    ("index_attempt", "total_docs_indexed"),
    ("index_attempt", "docs_removed_from_index"),
    ("index_attempt", "total_chunks"),
    ("index_attempt", "completed_batches"),
    ("index_attempt", "total_batches"),
    ("index_attempt", "total_failures_batch_level"),
    ("index_attempt", "time_created"),
    ("index_attempt", "time_started"),
    ("index_attempt", "time_updated"),
    ("index_attempt", "error_msg"),
    ("index_attempt", "from_beginning"),
    ("index_attempt", "poll_range_start"),
    ("index_attempt", "poll_range_end"),
    ("index_attempt", "last_heartbeat_time"),
    ("index_attempt", "heartbeat_counter"),
    ("index_attempt", "cancellation_requested"),
    ("index_attempt_errors", "id"),
    ("index_attempt_errors", "index_attempt_id"),
    ("index_attempt_errors", "connector_credential_pair_id"),
    ("index_attempt_errors", "document_id"),
    ("index_attempt_errors", "document_link"),
    ("index_attempt_errors", "failure_message"),
    ("index_attempt_errors", "error_type"),
    ("index_attempt_errors", "is_resolved"),
    ("index_attempt_errors", "time_created"),
    ("background_error", "id"),
    ("background_error", "message"),
    ("background_error", "time_created"),
    ("background_error", "cc_pair_id"),
    ("tag", "id"),
    ("tag", "tag_key"),
    ("tag", "tag_value"),
    ("tag", "source"),
    ("document__tag", "document_id"),
    ("document__tag", "tag_id"),
    ("search_settings", "id"),
    ("search_settings", "index_name"),
    ("search_settings", "status"),
    ("search_settings", "model_name"),
    ("search_settings", "model_dim"),
    ("search_settings", "query_prefix"),
    ("search_settings", "passage_prefix"),
];

/// Indexes from `ops/onyx_indexes.sql`. Their absence is a performance warning
/// reported by `/system/health`, never a startup failure — OVIS is correct
/// without them, just slow.
pub const EXPECTED_OVIS_INDEXES: &[&str] = &[
    "ix_ovis_document_updated",
    "ix_ovis_document_chunk_count",
    "ix_ovis_document_chunk_count_desc",
    "ix_ovis_document_boost",
    "ix_ovis_document_semantic_id_trgm",
    "ix_ovis_document_id_trgm",
    "ix_ovis_dcc_by_doc",
    "ix_ovis_document_tag_by_tag",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchemaProbe {
    /// `table.column` entries from [`REQUIRED_COLUMNS`] that are not in the
    /// live schema.
    pub missing_columns: Vec<String>,
    /// Tables with a restricting foreign key onto `document(id)` that the delete
    /// sweep does not clear. Non-empty means delete must refuse with
    /// `501 SCHEMA_MISMATCH` rather than fail halfway through a transaction.
    pub unhandled_fk_children: Vec<String>,
    /// Expected OVIS support indexes that do not exist.
    pub missing_indexes: Vec<String>,
}

impl SchemaProbe {
    /// Whether every column we read is present and the delete sweep is complete.
    /// Missing *indexes* deliberately do not affect this.
    pub fn is_ok(&self) -> bool {
        self.missing_columns.is_empty() && self.unhandled_fk_children.is_empty()
    }

    /// Does this probe cover `table.column`? Endpoints call this to decide
    /// between answering and returning `501 SCHEMA_MISMATCH`.
    pub fn has_column(&self, table: &str, column: &str) -> bool {
        let key = format!("{table}.{column}");
        !self.missing_columns.contains(&key)
    }
}

/// Run the whole probe. One query per concern, three round-trips total.
pub async fn probe_schema(pool: &PgPool) -> CoreResult<SchemaProbe> {
    let missing_columns = probe_columns(pool).await?;
    let unhandled_fk_children = probe_document_fk_children(pool).await?;
    let missing_indexes = probe_indexes(pool).await?;

    if !missing_columns.is_empty() {
        tracing::error!(
            missing = ?missing_columns,
            "Onyx schema is missing columns OVIS reads; affected endpoints will return \
             501 SCHEMA_MISMATCH. This usually means an Onyx upgrade moved something."
        );
    }
    if !unhandled_fk_children.is_empty() {
        tracing::error!(
            tables = ?unhandled_fk_children,
            "new foreign keys reference document(id) that the delete sweep does not clear; \
             document delete is disabled until DOCUMENT_FK_CHILD_TABLES covers them"
        );
    }
    if !missing_indexes.is_empty() {
        tracing::warn!(
            missing = ?missing_indexes,
            "OVIS support indexes are absent; list and search will be far slower than \
             their budgets. Apply ops/onyx_indexes.sql."
        );
    }

    Ok(SchemaProbe {
        missing_columns,
        unhandled_fk_children,
        missing_indexes,
    })
}

async fn probe_columns(pool: &PgPool) -> CoreResult<Vec<String>> {
    let tables: BTreeSet<&str> = REQUIRED_COLUMNS.iter().map(|(t, _)| *t).collect();
    let table_list: Vec<String> = tables.iter().map(|t| t.to_string()).collect();

    let rows = sqlx::query(
        "SELECT table_name, column_name FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = ANY($1)",
    )
    .bind(&table_list)
    .fetch_all(pool)
    .await?;

    let present: BTreeSet<String> = rows
        .into_iter()
        .map(|r| {
            let t: String = r.get("table_name");
            let c: String = r.get("column_name");
            format!("{t}.{c}")
        })
        .collect();

    Ok(REQUIRED_COLUMNS
        .iter()
        .map(|(t, c)| format!("{t}.{c}"))
        .filter(|key| !present.contains(key))
        .collect())
}

/// Find restricting foreign keys onto `document(id)` that the delete sweep does
/// not handle.
///
/// `CASCADE` and `SET NULL` children are excluded: Postgres deals with those.
pub async fn probe_document_fk_children(pool: &PgPool) -> CoreResult<Vec<String>> {
    let rows = sqlx::query(
        "SELECT src.relname AS child_table, \
                (SELECT string_agg(a.attname, ',' ORDER BY x.ord) \
                   FROM unnest(c.conkey) WITH ORDINALITY x(att, ord) \
                   JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = x.att \
                ) AS child_columns \
         FROM pg_constraint c \
         JOIN pg_class src ON src.oid = c.conrelid \
         JOIN pg_class tgt ON tgt.oid = c.confrelid \
         JOIN pg_namespace n ON n.oid = tgt.relnamespace \
         WHERE c.contype = 'f' \
           AND n.nspname = 'public' \
           AND tgt.relname = 'document' \
           AND c.confdeltype IN ('a', 'r')",
    )
    .fetch_all(pool)
    .await?;

    let handled: BTreeSet<(String, String)> = DOCUMENT_FK_CHILD_TABLES
        .iter()
        .map(|(t, c)| (t.to_string(), c.to_string()))
        .collect();

    let mut unhandled = Vec::new();
    for row in rows {
        let table: String = row.get("child_table");
        let columns: Option<String> = row.get("child_columns");
        let column = columns.unwrap_or_default();
        if !handled.contains(&(table.clone(), column.clone())) {
            unhandled.push(format!("{table}.{column}"));
        }
    }
    unhandled.sort();
    Ok(unhandled)
}

async fn probe_indexes(pool: &PgPool) -> CoreResult<Vec<String>> {
    let expected: Vec<String> = EXPECTED_OVIS_INDEXES.iter().map(|s| s.to_string()).collect();
    let rows = sqlx::query(
        // An interrupted CREATE INDEX CONCURRENTLY leaves an invalid index
        // behind that the planner ignores, so `indisvalid` is part of "exists".
        "SELECT c.relname FROM pg_class c \
         JOIN pg_index i ON i.indexrelid = c.oid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND i.indisvalid AND c.relname = ANY($1)",
    )
    .bind(&expected)
    .fetch_all(pool)
    .await?;

    let present: BTreeSet<String> = rows.into_iter().map(|r| r.get("relname")).collect();
    Ok(EXPECTED_OVIS_INDEXES
        .iter()
        .filter(|name| !present.contains(**name))
        .map(|name| name.to_string())
        .collect())
}

// ---------------------------------------------------------------------------
// search_settings — the live index name and embedding model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSettings {
    pub id: i32,
    pub index_name: String,
    pub model_name: String,
    pub model_dim: i32,
    /// Prefix Onyx prepends to a query before embedding it. Empty for the
    /// current arctic-embed row; still read rather than assumed, because
    /// embedding a query without the model's expected prefix silently degrades
    /// retrieval quality.
    pub query_prefix: String,
    pub passage_prefix: String,
}

/// Read the one `search_settings` row with `status = 'PRESENT'`.
///
/// Never use the `danswer_chunk*` wildcard instead of this: during an Onyx
/// re-embed a second index exists, and a wildcard delete would hit both.
pub async fn load_search_settings(pool: &PgPool) -> CoreResult<SearchSettings> {
    let row = sqlx::query(
        "SELECT id, index_name, model_name, model_dim, query_prefix, passage_prefix \
         FROM public.search_settings \
         WHERE status = 'PRESENT' \
         ORDER BY id DESC \
         LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        CoreError::SchemaMismatch(
            "no search_settings row has status='PRESENT'; cannot resolve the OpenSearch \
             index name"
                .into(),
        )
    })?;

    Ok(SearchSettings {
        id: row.get("id"),
        index_name: row.get("index_name"),
        model_name: row.get("model_name"),
        model_dim: row.get("model_dim"),
        query_prefix: row.get("query_prefix"),
        passage_prefix: row.get("passage_prefix"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_columns_have_no_duplicates() {
        let set: BTreeSet<(&str, &str)> = REQUIRED_COLUMNS.iter().copied().collect();
        assert_eq!(
            set.len(),
            REQUIRED_COLUMNS.len(),
            "REQUIRED_COLUMNS has duplicate entries"
        );
    }

    #[test]
    fn required_columns_cover_every_fk_child_used_by_delete() {
        // The delete sweep names tables directly; at minimum the ones that also
        // appear in read queries must be probed.
        assert!(REQUIRED_COLUMNS.contains(&("document__tag", "document_id")));
        assert!(REQUIRED_COLUMNS
            .contains(&("document_by_connector_credential_pair", "id")));
    }

    #[test]
    fn probe_ok_ignores_missing_indexes_but_not_missing_columns() {
        let perf_only = SchemaProbe {
            missing_indexes: vec!["ix_ovis_document_updated".into()],
            ..Default::default()
        };
        assert!(perf_only.is_ok(), "a missing index is a warning, not a fault");

        let broken = SchemaProbe {
            missing_columns: vec!["document.chunk_count".into()],
            ..Default::default()
        };
        assert!(!broken.is_ok());
        assert!(!broken.has_column("document", "chunk_count"));
        assert!(broken.has_column("document", "boost"));

        let unswept = SchemaProbe {
            unhandled_fk_children: vec!["document_shiny_new_thing.document_id".into()],
            ..Default::default()
        };
        assert!(!unswept.is_ok());
    }

    #[test]
    fn expected_indexes_match_the_ops_migration() {
        // Anything added to ops/onyx_indexes.sql must be probed, or /system/health
        // will report "all present" while the query is doing a seq scan.
        let sql = include_str!("../../../../ops/onyx_indexes.sql");
        for name in EXPECTED_OVIS_INDEXES {
            assert!(
                sql.contains(name),
                "{name} is expected by the probe but absent from ops/onyx_indexes.sql"
            );
        }
        for line in sql.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("CREATE INDEX CONCURRENTLY IF NOT EXISTS ") {
                let name = rest.split_whitespace().next().unwrap();
                assert!(
                    EXPECTED_OVIS_INDEXES.contains(&name),
                    "{name} is created by the migration but not probed"
                );
            }
        }
    }
}
