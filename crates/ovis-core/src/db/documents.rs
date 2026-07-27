//! The document list/detail/edit/delete path — the hot path of the whole app.
//!
//! Two rules make it fast, and they are the entire reason this module exists:
//!
//! * **`chunk_count` comes from `document.chunk_count`.** The list path makes
//!   zero OpenSearch calls. The old code issued one sequential OpenSearch search
//!   per row (50 per page, 1000 per SSE stream), each downloading full 768-dim
//!   embedding arrays just to call `.len()` on the result.
//! * **Ordering and paging are index-shaped.** Every `ORDER BY` in
//!   [`SortOrder::order_by`] has a matching index in `ops/onyx_indexes.sql`,
//!   including tie-break direction, and deep pages use keyset cursors rather
//!   than `OFFSET`.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use crate::api_types::{DeleteOutcome, PageDetail, PageListItem, TagKv};
use crate::cursor::{Cursor, SortOrder};
use crate::error::{CoreError, CoreResult};
use crate::search::OsClient;

/// Tables with a `NO ACTION`/`RESTRICT` foreign key onto `document(id)`, in the
/// order they must be cleared. Postgres handles the `CASCADE` children
/// (`persona__document`, `opensearch_document_migration_record`) and the
/// `SET NULL` one (`hierarchy_node`) by itself.
///
/// The old delete cleared only `document_by_connector_credential_pair`, so
/// deleting any tagged document failed outright — and 444,793 tag links exist.
///
/// [`super::probe::probe_document_fk_children`] compares this list against
/// `pg_constraint` at startup. Anything new that Onyx adds turns delete into a
/// `501 SCHEMA_MISMATCH` naming the table, instead of an error mid-transaction.
pub const DOCUMENT_FK_CHILD_TABLES: &[(&str, &str)] = &[
    // Relationships first: they FK both `document` and the entity tables below.
    ("kg_relationship", "source_document"),
    ("kg_relationship_extraction_staging", "source_document"),
    ("kg_entity", "document_id"),
    ("kg_entity_extraction_staging", "document_id"),
    ("chunk_stats", "document_id"),
    ("document__tag", "document_id"),
    ("document_retrieval_feedback", "document_id"),
    // Connector attribution last, immediately before `document` itself.
    ("document_by_connector_credential_pair", "id"),
];

/// cc-pair states in which Onyx will crawl again on schedule, so a deleted
/// document is liable to come back (web `refresh_freq` is 30 days here).
const RECRAWLING_STATES: [&str; 2] = ["ACTIVE", "INITIAL_INDEXING"];

/// Beyond this many rows, `page * limit` offset paging is refused: the database
/// would materialise and discard every skipped row. Clients switch to
/// `cursor` (or to filtering) instead — jumping to page 30,000 is not a real
/// workflow.
pub const MAX_OFFSET_DEPTH: i64 = 50_000;

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentFilter {
    /// Substring match against title and id/URL. Served by the trigram indexes.
    pub search: Option<String>,
    pub connector_id: Option<i32>,
    /// Matched case-insensitively against `connector.source` (stored upper-case).
    pub source: Option<String>,
    pub hidden: Option<bool>,
    pub chunk_min: Option<i32>,
    pub chunk_max: Option<i32>,
    pub updated_after: Option<DateTime<Utc>>,
    pub updated_before: Option<DateTime<Utc>>,
}

impl DocumentFilter {
    /// True when no predicate is set, i.e. the grand total. That is the one case
    /// where an exact `count(*)` is too expensive to run inline.
    pub fn is_unfiltered(&self) -> bool {
        *self == DocumentFilter::default()
    }

    /// True when the filter needs the connector tables joined at all.
    pub fn touches_connector(&self) -> bool {
        self.connector_id.is_some() || self.source.is_some()
    }
}

/// SQL fragment shared by list and count, covering every predicate *except* the
/// connector/source one — that is handled by [`ConnectorPlan`], which also picks
/// the shape of the whole query. `d` must be aliased to `public.document`.
///
/// Everything caller-supplied is bound, never interpolated.
fn push_filters<'a>(
    qb: &mut QueryBuilder<'a, Postgres>,
    f: &DocumentFilter,
    plan: &ConnectorPlan,
) {
    qb.push(" WHERE TRUE");

    if let Some(term) = &f.search {
        let pattern = format!("%{}%", escape_like(term));
        qb.push(" AND (d.semantic_id ILIKE ");
        qb.push_bind(pattern.clone());
        qb.push(" ESCAPE '\\' OR d.id ILIKE ");
        qb.push_bind(pattern);
        qb.push(" ESCAPE '\\')");
    }

    // In the Broad plan the connector predicate rides along as a semi-join:
    // EXISTS rather than a real join, so there are no duplicate rows to DISTINCT
    // away and no double-counting of the 3.2k documents that belong to several
    // connectors. In the Selective plan the id set is already the driving table.
    if let ConnectorPlan::Broad(connector_ids) = plan {
        qb.push(
            " AND EXISTS (SELECT 1 FROM public.document_by_connector_credential_pair z \
             WHERE z.id = d.id AND z.connector_id = ANY(",
        );
        qb.push_bind(connector_ids.clone());
        qb.push("))");
    }

    if let Some(hidden) = f.hidden {
        qb.push(" AND d.hidden = ");
        qb.push_bind(hidden);
    }

    // NULL chunk_count means "Onyx has not recorded a count", which is not the
    // same as zero. SQL's NULL semantics exclude those rows from both bounds,
    // which is the honest answer.
    if let Some(min) = f.chunk_min {
        qb.push(" AND d.chunk_count >= ");
        qb.push_bind(min);
    }
    if let Some(max) = f.chunk_max {
        qb.push(" AND d.chunk_count <= ");
        qb.push_bind(max);
    }

    if let Some(after) = f.updated_after {
        qb.push(" AND COALESCE(d.doc_updated_at, d.last_modified) >= ");
        qb.push_bind(after);
    }
    if let Some(before) = f.updated_before {
        qb.push(" AND COALESCE(d.doc_updated_at, d.last_modified) <= ");
        qb.push_bind(before);
    }
}

// ---------------------------------------------------------------------------
// Connector / source filtering
// ---------------------------------------------------------------------------

/// Match-set size at which the query flips from "drive off the connector" to
/// "drive off the recency index".
///
/// Measured on gamma (1.65M documents). The two shapes fail in opposite
/// directions:
///
/// | filter | recency-index-driven | connector-driven |
/// |---|---|---|
/// | `source=WEB` (1.66M docs) | **0.5 ms** | 3.0 s |
/// | connector 4 (105k docs)   | **107 ms** | 384 ms |
/// | connector 92 (15k docs)   | 140 ms | **202 ms** — comparable |
/// | `source=GITHUB` (1.7k)    | 317 ms | **~10 ms** |
/// | `source=WIKIPEDIA` (0)    | 2.0 s | **39 ms** |
///
/// Postgres's own planner picks correctly at the extremes but not in between: it
/// has no statistics that connect a connector id to where that connector's
/// documents sit in the recency ordering, so for a rare source it scans all
/// 1.65M index entries looking for 50 matches. Choosing the shape from a bounded
/// count is cheap (≤7 ms, see [`plan_connector_filter`]) and removes the 2-3 s
/// cases entirely.
pub const CONNECTOR_SELECTIVITY_THRESHOLD: i64 = 20_000;

/// How to apply the connector/source predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorPlan {
    /// No connector or source filter: drive straight off the sort index.
    Unfiltered,
    /// Few enough matching documents that enumerating them is cheaper than
    /// scanning the sort index for them. An empty id list means "nothing can
    /// match", which this shape answers in microseconds instead of scanning.
    Selective(Vec<i32>),
    /// Enough matching documents that the sort index will hit 50 of them
    /// quickly; apply the predicate as a semi-join.
    Broad(Vec<i32>),
}

impl ConnectorPlan {
    pub fn is_unfiltered(&self) -> bool {
        matches!(self, ConnectorPlan::Unfiltered)
    }
}

/// Resolve `connector_id` / `source` into a connector-id set and decide which
/// query shape to use.
///
/// Two cheap queries: one over `connector` (332 rows) and one bounded count over
/// `document_by_connector_credential_pair` that stops at the threshold.
pub async fn plan_connector_filter(
    pool: &PgPool,
    filter: &DocumentFilter,
) -> CoreResult<ConnectorPlan> {
    if !filter.touches_connector() {
        return Ok(ConnectorPlan::Unfiltered);
    }

    // Onyx stores source as the upper-case enum name; accept either casing from
    // callers without wrapping the column in `upper()`, which would throw away
    // the planner's statistics on it.
    let source_variants: Option<Vec<String>> = filter.source.as_ref().map(|s| {
        let upper = s.to_uppercase();
        let lower = s.to_lowercase();
        if upper == lower {
            vec![upper]
        } else {
            vec![upper, lower]
        }
    });

    let connector_ids: Vec<i32> = sqlx::query_scalar(
        "SELECT c.id FROM public.connector c \
         WHERE ($1::int IS NULL OR c.id = $1) \
           AND ($2::text[] IS NULL OR c.source = ANY($2))",
    )
    .bind(filter.connector_id)
    .bind(source_variants.as_ref())
    .fetch_all(pool)
    .await?;

    if connector_ids.is_empty() {
        return Ok(ConnectorPlan::Selective(Vec::new()));
    }

    let matches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ( \
             SELECT 1 FROM public.document_by_connector_credential_pair z \
             WHERE z.connector_id = ANY($1) \
             LIMIT $2 \
         ) bounded",
    )
    .bind(&connector_ids)
    .bind(CONNECTOR_SELECTIVITY_THRESHOLD + 1)
    .fetch_one(pool)
    .await?;

    Ok(if matches > CONNECTOR_SELECTIVITY_THRESHOLD {
        ConnectorPlan::Broad(connector_ids)
    } else {
        ConnectorPlan::Selective(connector_ids)
    })
}

/// Escape the LIKE metacharacters so a search for `100%` or `a_b` means what the
/// user typed. Paired with `ESCAPE '\'` at the call site.
fn escape_like(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 4);
    for ch in term.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// The keyset seek predicate: "strictly after this row, in this sort order".
///
/// Output-column aliases (`sort_ts`) are not visible in `WHERE`, so the recency
/// sorts repeat the `COALESCE` expression — which is exactly the expression the
/// index is built on, so it still seeks rather than scans.
fn push_keyset<'a>(qb: &mut QueryBuilder<'a, Postgres>, sort: SortOrder, cursor: &Cursor) {
    match sort {
        SortOrder::UpdatedDesc => {
            qb.push(" AND (COALESCE(d.doc_updated_at, d.last_modified), d.id) < (");
            qb.push_bind(cursor.ts);
            qb.push(", ");
            qb.push_bind(cursor.id.clone());
            qb.push(")");
        }
        SortOrder::UpdatedAsc => {
            qb.push(" AND (COALESCE(d.doc_updated_at, d.last_modified), d.id) > (");
            qb.push_bind(cursor.ts);
            qb.push(", ");
            qb.push_bind(cursor.id.clone());
            qb.push(")");
        }
        // chunk_count is nullable and both chunk sorts are NULLS LAST, so the
        // null tail is a separate region of the ordering and needs saying.
        SortOrder::ChunksDesc | SortOrder::ChunksAsc => {
            let cmp = if sort == SortOrder::ChunksDesc {
                "<"
            } else {
                ">"
            };
            match cursor.n {
                Some(n) => {
                    qb.push(" AND (d.chunk_count IS NULL OR d.chunk_count ");
                    qb.push(cmp);
                    qb.push(" ");
                    qb.push_bind(n);
                    qb.push(" OR (d.chunk_count = ");
                    qb.push_bind(n);
                    qb.push(" AND d.id < ");
                    qb.push_bind(cursor.id.clone());
                    qb.push("))");
                }
                None => {
                    qb.push(" AND d.chunk_count IS NULL AND d.id < ");
                    qb.push_bind(cursor.id.clone());
                }
            }
        }
        SortOrder::IdAsc => {
            qb.push(" AND d.id > ");
            qb.push_bind(cursor.id.clone());
        }
        SortOrder::IdDesc => {
            qb.push(" AND d.id < ");
            qb.push_bind(cursor.id.clone());
        }
        SortOrder::BoostDesc => {
            qb.push(" AND (d.boost, d.id) < (");
            qb.push_bind(cursor.n.unwrap_or(0));
            qb.push(", ");
            qb.push_bind(cursor.id.clone());
            qb.push(")");
        }
    }
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// Columns every list row carries.
const LIST_COLUMNS: &str = "\
SELECT d.id, \
       d.semantic_id, \
       d.link, \
       COALESCE(d.doc_updated_at, d.last_modified) AS sort_ts, \
       d.doc_updated_at, \
       d.last_modified, \
       d.chunk_count, \
       d.boost, \
       d.hidden, \
       d.doc_metadata AS metadata, \
       cx.connector_id, \
       cx.connector_name, \
       cx.connector_source ";

/// Connector attribution: a lateral pick of the lowest-numbered connector that
/// indexed the document. Deterministic, unlike the arbitrary winner the old
/// `DISTINCT ON` produced.
const CONNECTOR_LATERAL: &str = "\
LEFT JOIN LATERAL ( \
    SELECT c.id AS connector_id, c.name AS connector_name, c.source AS connector_source \
    FROM public.document_by_connector_credential_pair dcc \
    JOIN public.connector c ON c.id = dcc.connector_id \
    WHERE dcc.id = d.id \
    ORDER BY c.id \
    LIMIT 1 \
) cx ON TRUE";

/// `FROM` for the recency-index-driven shapes.
const FROM_DOCUMENT: &str = "FROM public.document d ";

/// `FROM` for the connector-driven shape: enumerate the matching document ids
/// first, then join. The `DISTINCT` both deduplicates multi-credential rows and
/// keeps Postgres from pulling the subquery up into the outer join, which is what
/// leads it to a full sequential scan of `document`.
const FROM_CONNECTOR_IDS_HEAD: &str = "\
FROM (SELECT DISTINCT z.id \
      FROM public.document_by_connector_credential_pair z \
      WHERE z.connector_id = ANY(";
const FROM_CONNECTOR_IDS_TAIL: &str = ")) m JOIN public.document d ON d.id = m.id ";

fn push_from<'a>(qb: &mut QueryBuilder<'a, Postgres>, plan: &ConnectorPlan, with_lateral: bool) {
    match plan {
        ConnectorPlan::Unfiltered | ConnectorPlan::Broad(_) => {
            qb.push(FROM_DOCUMENT);
        }
        ConnectorPlan::Selective(connector_ids) => {
            qb.push(FROM_CONNECTOR_IDS_HEAD);
            qb.push_bind(connector_ids.clone());
            qb.push(FROM_CONNECTOR_IDS_TAIL);
        }
    }
    if with_lateral {
        qb.push(CONNECTOR_LATERAL);
    }
}

#[derive(sqlx::FromRow)]
struct ListRow {
    id: String,
    semantic_id: String,
    link: Option<String>,
    sort_ts: DateTime<Utc>,
    doc_updated_at: Option<DateTime<Utc>>,
    last_modified: DateTime<Utc>,
    chunk_count: Option<i32>,
    boost: i32,
    hidden: bool,
    metadata: Option<serde_json::Value>,
    connector_id: Option<i32>,
    connector_name: Option<String>,
    connector_source: Option<String>,
}

impl From<ListRow> for PageListItem {
    fn from(r: ListRow) -> Self {
        PageListItem {
            id: r.id,
            semantic_id: r.semantic_id,
            link: r.link,
            updated_at: r.sort_ts,
            doc_updated_at: r.doc_updated_at,
            last_modified: r.last_modified,
            chunk_count: r.chunk_count,
            boost: r.boost,
            hidden: r.hidden,
            connector_id: r.connector_id,
            connector_name: r.connector_name,
            connector_source: r.connector_source,
            metadata: r.metadata,
        }
    }
}

/// Where a list request wants to start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Position<'a> {
    /// Rows to skip. Bounded by [`MAX_OFFSET_DEPTH`].
    ///
    /// An *offset*, not a page number: callers fetch `limit + 1` rows to detect
    /// a next page, and deriving the offset from that inflated limit would skip
    /// one extra row per page.
    Offset(i64),
    /// Opaque keyset token, already decoded and sort-checked.
    After(&'a Cursor),
}

impl Default for Position<'_> {
    fn default() -> Self {
        Position::Offset(0)
    }
}

/// Rows to skip for a 1-based page number at a given page size.
pub fn offset_for_page(page: i64, page_size: i64) -> i64 {
    (page.max(1) - 1).saturating_mul(page_size.max(1))
}

/// One page of documents, ordered by `sort`.
///
/// `plan` comes from [`plan_connector_filter`]; the caller resolves it once and
/// can reuse it for the matching [`count_documents`] call.
pub async fn list_documents(
    pool: &PgPool,
    filter: &DocumentFilter,
    plan: &ConnectorPlan,
    sort: SortOrder,
    position: Position<'_>,
    limit: i64,
) -> CoreResult<Vec<PageListItem>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(LIST_COLUMNS);
    push_from(&mut qb, plan, true);
    push_filters(&mut qb, filter, plan);

    let offset = match position {
        Position::After(cursor) => {
            push_keyset(&mut qb, sort, cursor);
            0
        }
        Position::Offset(offset) => {
            let offset = offset.max(0);
            if offset.saturating_add(limit) > MAX_OFFSET_DEPTH {
                return Err(CoreError::Invalid(format!(
                    "an offset of {offset} rows is deeper than the {MAX_OFFSET_DEPTH}-row \
                     bound; use the cursor from `next_cursor`, or narrow the filter"
                )));
            }
            offset
        }
    };

    qb.push(" ORDER BY ").push(sort.order_by());
    qb.push(" LIMIT ").push_bind(limit);
    if offset > 0 {
        qb.push(" OFFSET ").push_bind(offset);
    }

    let rows: Vec<ListRow> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(PageListItem::from).collect())
}

/// Exact `count(*)` for a filter. Cache this; it is the expensive half of a list
/// response, which is why the caller runs it concurrently with the row fetch
/// rather than before it.
pub async fn count_documents(
    pool: &PgPool,
    filter: &DocumentFilter,
    plan: &ConnectorPlan,
) -> CoreResult<i64> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT count(*) ");
    // No connector lateral: counting does not need attribution.
    push_from(&mut qb, plan, false);
    push_filters(&mut qb, filter, plan);
    let count: i64 = qb.build_query_scalar().fetch_one(pool).await?;
    Ok(count)
}

/// Planner row estimate for `public.document`, used for the unfiltered grand
/// total so a list request never waits on a full-table count. Responses that use
/// it set `total_exact: false`.
pub async fn estimate_total_documents(pool: &PgPool) -> CoreResult<i64> {
    let estimate: Option<i64> = sqlx::query_scalar(
        "SELECT GREATEST(reltuples, 0)::bigint FROM pg_class \
         WHERE oid = 'public.document'::regclass",
    )
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(estimate.unwrap_or(0))
}

/// Hydrate search hits: one round-trip for every document id in the result set.
///
/// `connector_ids`, when present, also filters — the chunk index carries no
/// connector field, so a connector-scoped search can only be narrowed here.
pub async fn documents_by_ids(
    pool: &PgPool,
    ids: &[String],
    connector_ids: Option<&[i32]>,
) -> CoreResult<Vec<PageListItem>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(LIST_COLUMNS);
    qb.push(FROM_DOCUMENT);
    qb.push(CONNECTOR_LATERAL);
    qb.push(" WHERE d.id = ANY(");
    qb.push_bind(ids.to_vec());
    qb.push(")");
    if let Some(connector_ids) = connector_ids {
        qb.push(
            " AND EXISTS (SELECT 1 FROM public.document_by_connector_credential_pair z \
             WHERE z.id = d.id AND z.connector_id = ANY(",
        );
        qb.push_bind(connector_ids.to_vec());
        qb.push("))");
    }
    let rows: Vec<ListRow> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(PageListItem::from).collect())
}

// ---------------------------------------------------------------------------
// Detail
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct DetailRow {
    id: String,
    semantic_id: String,
    link: Option<String>,
    sort_ts: DateTime<Utc>,
    doc_updated_at: Option<DateTime<Utc>>,
    last_modified: DateTime<Utc>,
    last_synced: Option<DateTime<Utc>>,
    chunk_count: Option<i32>,
    boost: i32,
    hidden: bool,
    metadata: Option<serde_json::Value>,
    primary_owners: Option<Vec<String>>,
    secondary_owners: Option<Vec<String>>,
    content_hash: Option<String>,
    from_ingestion_api: Option<bool>,
    connector_id: Option<i32>,
    connector_name: Option<String>,
    connector_source: Option<String>,
    cc_pair_id: Option<i32>,
    cc_pair_status: Option<String>,
}

const DETAIL_SQL: &str = "\
SELECT d.id, \
       d.semantic_id, \
       d.link, \
       COALESCE(d.doc_updated_at, d.last_modified) AS sort_ts, \
       d.doc_updated_at, \
       d.last_modified, \
       d.last_synced, \
       d.chunk_count, \
       d.boost, \
       d.hidden, \
       d.doc_metadata AS metadata, \
       d.primary_owners, \
       d.secondary_owners, \
       d.content_hash, \
       d.from_ingestion_api, \
       cx.connector_id, \
       cx.connector_name, \
       cx.connector_source, \
       cc.id AS cc_pair_id, \
       cc.status AS cc_pair_status \
FROM public.document d \
LEFT JOIN LATERAL ( \
    SELECT c.id AS connector_id, c.name AS connector_name, c.source AS connector_source, \
           dcc.connector_id AS dcc_cid, dcc.credential_id AS dcc_crid \
    FROM public.document_by_connector_credential_pair dcc \
    JOIN public.connector c ON c.id = dcc.connector_id \
    WHERE dcc.id = d.id \
    ORDER BY c.id \
    LIMIT 1 \
) cx ON TRUE \
LEFT JOIN public.connector_credential_pair cc \
       ON cc.connector_id = cx.dcc_cid AND cc.credential_id = cx.dcc_crid \
WHERE d.id = $1";

/// Metadata detail for one document. `Ok(None)` means there is genuinely no
/// `document` row — a database failure is an `Err`, never a synthesised
/// "missing" answer.
pub async fn get_document(pool: &PgPool, id: &str) -> CoreResult<Option<PageDetail>> {
    let row: Option<DetailRow> = sqlx::query_as(DETAIL_SQL)
        .bind(id)
        .fetch_optional(pool)
        .await?;

    let Some(r) = row else { return Ok(None) };

    let recrawl_risk = r
        .cc_pair_status
        .as_deref()
        .map(|s| RECRAWLING_STATES.contains(&s))
        .unwrap_or(false);

    Ok(Some(PageDetail {
        item: PageListItem {
            id: r.id,
            semantic_id: r.semantic_id,
            link: r.link,
            updated_at: r.sort_ts,
            doc_updated_at: r.doc_updated_at,
            last_modified: r.last_modified,
            chunk_count: r.chunk_count,
            boost: r.boost,
            hidden: r.hidden,
            connector_id: r.connector_id,
            connector_name: r.connector_name,
            connector_source: r.connector_source,
            metadata: r.metadata,
        },
        primary_owners: r.primary_owners,
        secondary_owners: r.secondary_owners,
        content_hash: r.content_hash,
        from_ingestion_api: r.from_ingestion_api,
        last_synced: r.last_synced,
        cc_pair_id: r.cc_pair_id,
        cc_pair_status: r.cc_pair_status,
        tags: Vec::new(),
        pg_row: true,
        recrawl_risk,
    }))
}

/// Tags attached to one document. Bounded — a pathological document should not
/// be able to produce a megabyte of tag JSON.
pub async fn get_document_tags(pool: &PgPool, id: &str, limit: i64) -> CoreResult<Vec<TagKv>> {
    let rows = sqlx::query(
        "SELECT t.tag_key, t.tag_value \
         FROM public.document__tag dt \
         JOIN public.tag t ON t.id = dt.tag_id \
         WHERE dt.document_id = $1 \
         ORDER BY t.tag_key, t.tag_value \
         LIMIT $2",
    )
    .bind(id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| TagKv {
            key: row.get("tag_key"),
            value: row.get("tag_value"),
        })
        .collect())
}

/// Whether the owning cc-pair would re-crawl this document. Used by the delete
/// path, which needs the answer *before* the row disappears.
pub async fn recrawl_risk(pool: &PgPool, id: &str) -> CoreResult<bool> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT cc.status \
         FROM public.document_by_connector_credential_pair dcc \
         JOIN public.connector_credential_pair cc \
           ON cc.connector_id = dcc.connector_id AND cc.credential_id = dcc.credential_id \
         WHERE dcc.id = $1 AND cc.status = ANY($2) \
         LIMIT 1",
    )
    .bind(id)
    .bind(RECRAWLING_STATES.map(String::from).to_vec())
    .fetch_optional(pool)
    .await?;
    Ok(status.is_some())
}

// ---------------------------------------------------------------------------
// Edit
// ---------------------------------------------------------------------------

/// Fields a `PATCH` may change in Postgres.
#[derive(Debug, Clone, Default)]
pub struct DocumentUpdate {
    pub semantic_id: Option<String>,
    pub boost: Option<i32>,
    pub hidden: Option<bool>,
    pub metadata_merge: Option<serde_json::Value>,
}

impl DocumentUpdate {
    pub fn is_empty(&self) -> bool {
        self.semantic_id.is_none()
            && self.boost.is_none()
            && self.hidden.is_none()
            && self.metadata_merge.is_none()
    }
}

/// Apply an edit. Returns rows affected (0 ⇒ no such document).
///
/// Two deliberate constraints, both of which the old CLI edit path violated:
///
/// * `doc_metadata` is **merged**, not replaced. The old code wrote the whole
///   object back, so any key it had not read was lost.
/// * `doc_updated_at` is **never touched**. It is Onyx's crawl timestamp, and
///   `last_modified > last_synced` is what drives Onyx's own sync detection via
///   the partial `ix_document_needs_sync` index. Writing it corrupts that
///   contract. (The `update_kg_entity_name_from_doc_trigger` trigger does fire
///   on `semantic_id` — that is Onyx's own bookkeeping and is fine.)
pub async fn update_document(
    pool: &PgPool,
    id: &str,
    update: &DocumentUpdate,
) -> CoreResult<u64> {
    let result = sqlx::query(
        "UPDATE public.document \
         SET semantic_id  = COALESCE($1::varchar, semantic_id), \
             boost        = COALESCE($2::int, boost), \
             hidden       = COALESCE($3::bool, hidden), \
             doc_metadata = CASE WHEN $4::jsonb IS NULL THEN doc_metadata \
                                 ELSE COALESCE(doc_metadata, '{}'::jsonb) || $4::jsonb END \
         WHERE id = $5",
    )
    .bind(update.semantic_id.as_deref())
    .bind(update.boost)
    .bind(update.hidden)
    .bind(update.metadata_merge.as_ref())
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// Cascading delete of one document: Postgres first (transactionally, with the
/// full FK child sweep), then the OpenSearch chunks.
///
/// Postgres commits before the index delete because the two systems have no
/// shared transaction. If the index delete then fails, the id is queued in
/// `ovis.pending_index_deletes` and the outcome says
/// `index_cleanup_pending: true` — a background drain retries it. The old code
/// acknowledged in a comment that chunks could orphan permanently, and did
/// nothing about it.
pub async fn delete_document_cascading(
    pool: &PgPool,
    os: &OsClient,
    index: &str,
    id: &str,
) -> CoreResult<DeleteOutcome> {
    // Read the recrawl answer before the row is gone.
    let recrawl_risk = recrawl_risk(pool, id).await.unwrap_or(false);

    let mut tx = pool.begin().await?;

    for (table, column) in DOCUMENT_FK_CHILD_TABLES {
        // Table and column names come from the const above, never from input.
        let sql = format!("DELETE FROM public.{table} WHERE {column} = $1");
        sqlx::query(&sql).bind(id).execute(&mut *tx).await?;
    }

    let deleted = sqlx::query("DELETE FROM public.document WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    if deleted == 0 {
        // Dropping the transaction rolls it back.
        return Err(CoreError::not_found("document", id));
    }
    tx.commit().await?;

    let (chunks_deleted, index_cleanup_pending) =
        match os.delete_document_chunks(index, id).await {
            Ok(n) => (n, false),
            Err(err) => {
                tracing::warn!(
                    document_id = %id,
                    error = %err,
                    "postgres delete committed but index cleanup failed; queueing for retry"
                );
                let _ = super::pending_deletes::enqueue(pool, id, &err.to_string()).await;
                (0, true)
            }
        };

    Ok(DeleteOutcome {
        pg_deleted: true,
        chunks_deleted,
        index_cleanup_pending,
        recrawl_risk,
    })
}

/// Postgres-only half of the delete, for the batch path which shares one
/// OpenSearch call across a chunk of ids.
pub async fn delete_document_pg_only(pool: &PgPool, id: &str) -> CoreResult<()> {
    let mut tx = pool.begin().await?;
    for (table, column) in DOCUMENT_FK_CHILD_TABLES {
        let sql = format!("DELETE FROM public.{table} WHERE {column} = $1");
        sqlx::query(&sql).bind(id).execute(&mut *tx).await?;
    }
    let deleted = sqlx::query("DELETE FROM public.document WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(CoreError::not_found("document", id));
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated SQL is asserted directly: these queries are the difference
    /// between a 0.6 ms and a 965 ms list request, and a silent regression in the
    /// predicate shape would not show up as a test failure anywhere else.
    fn built_sql(
        filter: &DocumentFilter,
        plan: &ConnectorPlan,
        sort: SortOrder,
        position: Position<'_>,
    ) -> String {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(LIST_COLUMNS);
        push_from(&mut qb, plan, true);
        push_filters(&mut qb, filter, plan);
        if let Position::After(c) = position {
            push_keyset(&mut qb, sort, c);
        }
        qb.push(" ORDER BY ").push(sort.order_by());
        qb.into_sql()
    }

    fn built_list_sql(filter: &DocumentFilter, sort: SortOrder, position: Position<'_>) -> String {
        built_sql(filter, &ConnectorPlan::Unfiltered, sort, position)
    }

    #[test]
    fn unfiltered_list_has_no_predicates_and_no_distinct() {
        let sql = built_list_sql(
            &DocumentFilter::default(),
            SortOrder::UpdatedDesc,
            Position::Offset(0),
        );
        assert!(sql.contains("WHERE TRUE"));
        assert!(!sql.contains("DISTINCT"), "no dedup pass should be needed");
        assert!(sql.contains("ORDER BY sort_ts DESC, d.id DESC"));
        assert!(
            sql.contains("LEFT JOIN LATERAL"),
            "connector attribution must be a deterministic lateral pick"
        );
        assert!(sql.contains("FROM public.document d"));
    }

    #[test]
    fn sort_is_recency_not_lexicographic_by_url() {
        // Regression for the DISTINCT ON (d.id) bug, which forced ORDER BY d.id
        // and silently made "newest first" mean "alphabetically by URL".
        let sql = built_list_sql(
            &DocumentFilter::default(),
            SortOrder::UpdatedDesc,
            Position::Offset(0),
        );
        // The lateral has its own ORDER BY c.id, so take the outer one.
        let order = sql.rsplit("ORDER BY").next().unwrap();
        assert!(
            order.trim_start().starts_with("sort_ts DESC"),
            "outer ordering was '{}'",
            order.trim()
        );
    }

    #[test]
    fn a_broad_connector_filter_becomes_a_semi_join_on_the_recency_scan() {
        let sql = built_sql(
            &DocumentFilter {
                source: Some("web".into()),
                ..Default::default()
            },
            &ConnectorPlan::Broad(vec![1, 2, 3]),
            SortOrder::UpdatedDesc,
            Position::Offset(0),
        );
        // One EXISTS, driven off the sort index; no join to dedup afterwards.
        assert_eq!(sql.matches("EXISTS (SELECT 1").count(), 1);
        assert!(sql.contains("z.connector_id = ANY("));
        assert!(sql.contains("FROM public.document d"));
        assert!(!sql.contains("DISTINCT"));
        // `upper()` on the column would discard the planner's statistics on
        // connector.source, which is what made a rare-source filter take 2 s.
        assert!(!sql.contains("upper(c"));
    }

    #[test]
    fn a_selective_connector_filter_drives_off_the_connector_instead() {
        let sql = built_sql(
            &DocumentFilter {
                connector_id: Some(42),
                ..Default::default()
            },
            &ConnectorPlan::Selective(vec![42]),
            SortOrder::UpdatedDesc,
            Position::Offset(0),
        );
        assert!(
            sql.contains("SELECT DISTINCT z.id"),
            "the selective shape enumerates the connector's documents: {sql}"
        );
        assert!(sql.contains("JOIN public.document d ON d.id = m.id"));
        assert!(
            !sql.contains("EXISTS (SELECT 1"),
            "the id set already is the filter"
        );
        assert!(sql.contains("ORDER BY sort_ts DESC, d.id DESC"));
    }

    #[test]
    fn an_impossible_connector_filter_still_produces_a_bounded_query() {
        // No connector matched the requested source: the shape must answer
        // "nothing" from an empty array rather than scanning 1.65M rows.
        let sql = built_sql(
            &DocumentFilter {
                source: Some("dropbox".into()),
                ..Default::default()
            },
            &ConnectorPlan::Selective(Vec::new()),
            SortOrder::UpdatedDesc,
            Position::Offset(0),
        );
        assert!(sql.contains("z.connector_id = ANY("));
        assert!(!sql.contains("EXISTS"));
    }

    #[test]
    fn counts_use_the_same_plan_but_skip_connector_attribution() {
        for plan in [
            ConnectorPlan::Unfiltered,
            ConnectorPlan::Broad(vec![7]),
            ConnectorPlan::Selective(vec![7]),
        ] {
            let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT count(*) ");
            push_from(&mut qb, &plan, false);
            push_filters(&mut qb, &DocumentFilter::default(), &plan);
            let sql = qb.into_sql();
            assert!(
                !sql.contains("LEFT JOIN LATERAL"),
                "counting does not need the connector lateral: {sql}"
            );
        }
    }

    #[test]
    fn search_filter_binds_the_pattern_and_never_interpolates() {
        let sql = built_list_sql(
            &DocumentFilter {
                search: Some("'; DROP TABLE document; --".into()),
                ..Default::default()
            },
            SortOrder::UpdatedDesc,
            Position::Offset(0),
        );
        assert!(
            !sql.contains("DROP TABLE"),
            "the term must be a bind parameter, not SQL text: {sql}"
        );
        assert!(sql.contains("d.semantic_id ILIKE $1"));
        assert!(sql.contains("d.id ILIKE $2"));
    }

    #[test]
    fn like_metacharacters_are_escaped() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("c:\\path"), "c:\\\\path");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn recency_keyset_uses_the_indexed_expression_not_the_alias() {
        // `sort_ts` is not visible in WHERE, and using the raw expression is
        // also what lets ix_ovis_document_updated serve the seek.
        let cursor = Cursor {
            sort: SortOrder::UpdatedDesc,
            ts: Some("2026-07-26T00:00:00Z".parse().unwrap()),
            n: None,
            id: "https://x/y".into(),
        };
        let sql = built_list_sql(
            &DocumentFilter::default(),
            SortOrder::UpdatedDesc,
            Position::After(&cursor),
        );
        assert!(sql.contains("(COALESCE(d.doc_updated_at, d.last_modified), d.id) < ("));
        assert!(!sql.contains("(sort_ts, d.id)"));
    }

    #[test]
    fn chunk_keyset_handles_the_null_tail_in_both_directions() {
        for (sort, cmp) in [
            (SortOrder::ChunksDesc, "<"),
            (SortOrder::ChunksAsc, ">"),
        ] {
            let with_value = Cursor {
                sort,
                ts: None,
                n: Some(7),
                id: "https://x/y".into(),
            };
            let sql = built_list_sql(
                &DocumentFilter::default(),
                sort,
                Position::After(&with_value),
            );
            assert!(
                sql.contains(&format!("d.chunk_count {cmp} $1")),
                "{}: {sql}",
                sort.as_str()
            );
            assert!(sql.contains("d.chunk_count IS NULL OR"));

            let in_null_tail = Cursor {
                sort,
                ts: None,
                n: None,
                id: "https://x/y".into(),
            };
            let sql = built_list_sql(
                &DocumentFilter::default(),
                sort,
                Position::After(&in_null_tail),
            );
            assert!(sql.contains("AND d.chunk_count IS NULL AND d.id < $1"));
            assert!(!sql.contains("IS NULL OR"));
        }
    }

    #[test]
    fn every_sort_produces_a_keyset_predicate() {
        for sort in [
            SortOrder::UpdatedDesc,
            SortOrder::UpdatedAsc,
            SortOrder::ChunksDesc,
            SortOrder::ChunksAsc,
            SortOrder::IdAsc,
            SortOrder::IdDesc,
            SortOrder::BoostDesc,
        ] {
            let cursor = Cursor {
                sort,
                ts: Some("2026-07-26T00:00:00Z".parse().unwrap()),
                n: Some(3),
                id: "https://x/y".into(),
            };
            let bare = built_list_sql(&DocumentFilter::default(), sort, Position::Offset(0));
            let seeking =
                built_list_sql(&DocumentFilter::default(), sort, Position::After(&cursor));
            assert!(
                seeking.len() > bare.len(),
                "{} produced no seek predicate",
                sort.as_str()
            );
        }
    }

    #[test]
    fn filter_emptiness_and_connector_touch_detection() {
        assert!(DocumentFilter::default().is_unfiltered());
        assert!(!DocumentFilter::default().touches_connector());

        let f = DocumentFilter {
            hidden: Some(false),
            ..Default::default()
        };
        assert!(!f.is_unfiltered());
        assert!(!f.touches_connector());

        let f = DocumentFilter {
            source: Some("web".into()),
            ..Default::default()
        };
        assert!(f.touches_connector());
    }

    #[test]
    fn fk_child_sweep_covers_every_no_action_child_and_orders_dcc_last() {
        let names: Vec<&str> = DOCUMENT_FK_CHILD_TABLES.iter().map(|(t, _)| *t).collect();
        for required in [
            "document__tag",
            "chunk_stats",
            "document_retrieval_feedback",
            "document_by_connector_credential_pair",
            "kg_entity",
            "kg_entity_extraction_staging",
            "kg_relationship",
            "kg_relationship_extraction_staging",
        ] {
            assert!(names.contains(&required), "missing FK child {required}");
        }
        // Relationships FK the entity tables, so they must be cleared first.
        let pos = |t: &str| names.iter().position(|n| *n == t).unwrap();
        assert!(pos("kg_relationship") < pos("kg_entity"));
        assert!(pos("kg_relationship_extraction_staging") < pos("kg_entity_extraction_staging"));
        assert_eq!(*names.last().unwrap(), "document_by_connector_credential_pair");
    }

    #[test]
    fn page_offsets_are_derived_from_the_page_size_not_the_fetch_size() {
        // Callers fetch limit+1 rows to detect a next page. Deriving the offset
        // from that inflated number skips one extra row per page, so page 2 of a
        // 10-row listing would start at row 12.
        assert_eq!(offset_for_page(1, 10), 0);
        assert_eq!(offset_for_page(2, 10), 10);
        assert_eq!(offset_for_page(3, 10), 20);
        // Degenerate inputs must not produce a negative offset.
        assert_eq!(offset_for_page(0, 10), 0);
        assert_eq!(offset_for_page(-3, 10), 0);
        assert_eq!(offset_for_page(2, 0), 1);
    }

    #[test]
    fn offset_depth_is_bounded() {
        assert!(offset_for_page(1, 50) + 50 <= MAX_OFFSET_DEPTH);
        let deep = (MAX_OFFSET_DEPTH / 50) + 2;
        assert!(offset_for_page(deep, 50) + 50 > MAX_OFFSET_DEPTH);
    }

    #[test]
    fn selectivity_threshold_keeps_both_shapes_bounded() {
        // Below the threshold the connector-driven shape sorts at most this many
        // rows; above it, the recency scan finds 50 matches within roughly
        // 1.65M/20k * 50 index entries. Both stay in the low hundreds of ms.
        assert_eq!(CONNECTOR_SELECTIVITY_THRESHOLD, 20_000);
    }
}
