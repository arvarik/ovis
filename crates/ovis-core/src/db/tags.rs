//! Tag facets — the filter-picker surface over 230k tags / 445k document links.

use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use crate::api_types::TagFacet;
use crate::error::CoreResult;

/// Facet counts, most-used first.
///
/// This is a grouped scan of `document__tag`, so it is always served from cache
/// (60 s TTL) rather than per keystroke. `key` narrows to one tag key, which is
/// what the "pick a value for author" step of a filter picker needs.
pub async fn list_facets(
    pool: &PgPool,
    key: Option<&str>,
    value_prefix: Option<&str>,
    limit: i64,
) -> CoreResult<Vec<TagFacet>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT t.tag_key, t.tag_value, count(*) AS doc_count \
         FROM public.document__tag dt \
         JOIN public.tag t ON t.id = dt.tag_id \
         WHERE TRUE",
    );
    if let Some(key) = key {
        qb.push(" AND t.tag_key = ");
        qb.push_bind(key.to_string());
    }
    if let Some(prefix) = value_prefix.filter(|p| !p.is_empty()) {
        qb.push(" AND t.tag_value ILIKE ");
        qb.push_bind(format!("{}%", prefix.replace(['%', '_'], "")));
    }
    qb.push(
        " GROUP BY t.tag_key, t.tag_value ORDER BY doc_count DESC, t.tag_key, t.tag_value LIMIT ",
    );
    qb.push_bind(limit);

    let rows = qb.build().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| TagFacet {
            key: r.get("tag_key"),
            value: r.get("tag_value"),
            doc_count: r.get("doc_count"),
        })
        .collect())
}

/// Distinct tag keys with how many distinct values each has. Cheap enough to
/// serve the top level of a filter picker.
pub async fn list_keys(pool: &PgPool, limit: i64) -> CoreResult<Vec<(String, i64)>> {
    let rows = sqlx::query(
        "SELECT tag_key, count(*) AS n FROM public.tag \
         GROUP BY tag_key ORDER BY n DESC, tag_key LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("tag_key"), r.get("n")))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facet_filters_are_bound_not_interpolated() {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT 1 WHERE TRUE");
        qb.push(" AND t.tag_key = ");
        qb.push_bind("author'; DROP TABLE tag; --".to_string());
        let sql = qb.into_sql();
        assert!(!sql.as_str().contains("DROP TABLE"));
        assert!(sql.as_str().contains("t.tag_key = $1"));
    }

    #[test]
    fn value_prefix_strips_like_wildcards() {
        // A user typing `%` should not turn the prefix filter into "match all".
        assert_eq!("a%b_c".replace(['%', '_'], ""), "abc");
    }
}
