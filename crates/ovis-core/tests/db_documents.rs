//! Integration tests for the document data layer, against the real Onyx DDL.
//!
//! Each test targets a specific defect from the audit. Where that matters the
//! test says which one, because "sorted by recency" is only interesting if you
//! know it used to be "sorted alphabetically by URL".

mod common;

use common::docs;
use ovis_core::cursor::{Cursor, SortOrder};
use ovis_core::db::documents::{self, ConnectorPlan, DocumentFilter, DocumentUpdate, Position};
use ovis_core::CoreError;

async fn list(
    pool: &sqlx::PgPool,
    filter: &DocumentFilter,
    sort: SortOrder,
    limit: i64,
) -> Vec<ovis_core::api_types::PageListItem> {
    let plan = documents::plan_connector_filter(pool, filter)
        .await
        .unwrap();
    documents::list_documents(pool, filter, &plan, sort, Position::Offset(0), limit)
        .await
        .unwrap()
}

#[tokio::test]
async fn default_sort_is_newest_first_not_alphabetical_by_url() {
    let Some(db) = common::seeded().await else {
        return common::skip("default_sort_is_newest_first_not_alphabetical_by_url");
    };
    let pool = db.pool.clone();

    let items = list(
        &pool,
        &DocumentFilter::default(),
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();

    // The seed's alphabetically-first document is deliberately the oldest. The
    // old `DISTINCT ON (d.id)` query forced `ORDER BY d.id`, so it came first.
    assert_ne!(
        ids.first(),
        Some(&docs::OLDEST),
        "listing is ordered by URL, not recency"
    );
    assert_eq!(ids.last(), Some(&docs::OLDEST), "oldest should sort last");
    assert_eq!(ids.first(), Some(&docs::NEWEST));

    let timestamps: Vec<_> = items.iter().map(|i| i.updated_at).collect();
    let mut sorted = timestamps.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(timestamps, sorted, "not monotonically newest-first");
}

#[tokio::test]
async fn recency_falls_back_to_last_modified_when_doc_updated_at_is_null() {
    let Some(db) = common::seeded().await else {
        return common::skip("recency_falls_back_to_last_modified_when_doc_updated_at_is_null");
    };
    let pool = db.pool.clone();

    let items = list(
        &pool,
        &DocumentFilter::default(),
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    let oldest = items.iter().find(|i| i.id == docs::OLDEST).unwrap();
    assert!(
        oldest.doc_updated_at.is_none(),
        "fixture expects a null crawl timestamp here"
    );
    assert_eq!(
        oldest.updated_at, oldest.last_modified,
        "updated_at must fall back to last_modified"
    );

    // And the one row that *has* a crawl timestamp uses it.
    let newest = items.iter().find(|i| i.id == docs::NEWEST).unwrap();
    assert_eq!(newest.updated_at, newest.doc_updated_at.unwrap());
}

#[tokio::test]
async fn chunk_count_comes_from_postgres_with_null_distinct_from_zero() {
    let Some(db) = common::seeded().await else {
        return common::skip("chunk_count_comes_from_postgres_with_null_distinct_from_zero");
    };
    let pool = db.pool.clone();

    let items = list(
        &pool,
        &DocumentFilter::default(),
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    let by_id = |id: &str| items.iter().find(|i| i.id == id).unwrap();

    assert_eq!(by_id(docs::MIDDLE).chunk_count, Some(12));
    assert_eq!(by_id(docs::STUB).chunk_count, Some(0));
    assert_eq!(
        by_id(docs::UNCOUNTED).chunk_count,
        None,
        "an uncounted document must report null, not zero"
    );
}

#[tokio::test]
async fn chunk_bounds_exclude_uncounted_documents() {
    let Some(db) = common::seeded().await else {
        return common::skip("chunk_bounds_exclude_uncounted_documents");
    };
    let pool = db.pool.clone();

    // The UI's "stubs" preset.
    let stubs = list(
        &pool,
        &DocumentFilter {
            chunk_min: Some(0),
            chunk_max: Some(0),
            ..Default::default()
        },
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    let ids: Vec<&str> = stubs.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, vec![docs::STUB], "only the zero-chunk document");
    assert!(
        !ids.contains(&docs::UNCOUNTED),
        "unknown is not zero; an uncounted document is not a stub"
    );

    // The "heavy" preset.
    let heavy = list(
        &pool,
        &DocumentFilter {
            chunk_min: Some(11),
            ..Default::default()
        },
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    assert_eq!(
        heavy.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec![docs::MIDDLE]
    );
}

#[tokio::test]
async fn every_sort_order_is_total_and_paginates_without_gaps_or_repeats() {
    let Some(db) = common::seeded().await else {
        return common::skip("every_sort_order_is_total_and_paginates_without_gaps_or_repeats");
    };
    let pool = db.pool.clone();

    let filter = DocumentFilter::default();
    let plan = documents::plan_connector_filter(&pool, &filter)
        .await
        .unwrap();
    let all = list(&pool, &filter, SortOrder::UpdatedDesc, 100).await;
    let total = all.len();
    assert!(total >= 10, "fixture should hold at least 10 documents");

    for sort in [
        SortOrder::UpdatedDesc,
        SortOrder::UpdatedAsc,
        SortOrder::ChunksDesc,
        SortOrder::ChunksAsc,
        SortOrder::IdAsc,
        SortOrder::IdDesc,
        SortOrder::BoostDesc,
    ] {
        // Walk the whole set two rows at a time via cursors.
        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<Cursor> = None;
        for _ in 0..(total + 5) {
            let position = match &cursor {
                Some(c) => Position::After(c),
                None => Position::Offset(0),
            };
            let page = documents::list_documents(&pool, &filter, &plan, sort, position, 2)
                .await
                .unwrap();
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|item| Cursor::after(sort, item));
            seen.extend(page.into_iter().map(|i| i.id));
        }

        assert_eq!(
            seen.len(),
            total,
            "{}: keyset paging visited {} of {} rows",
            sort.as_str(),
            seen.len(),
            total
        );
        let unique: std::collections::HashSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            total,
            "{}: keyset paging repeated a row",
            sort.as_str()
        );
    }
}

#[tokio::test]
async fn chunk_sorts_walk_the_null_tail_exactly_once() {
    let Some(db) = common::seeded().await else {
        return common::skip("chunk_sorts_walk_the_null_tail_exactly_once");
    };
    let pool = db.pool.clone();

    let filter = DocumentFilter::default();
    let plan = ConnectorPlan::Unfiltered;

    for sort in [SortOrder::ChunksDesc, SortOrder::ChunksAsc] {
        let page = documents::list_documents(&pool, &filter, &plan, sort, Position::Offset(0), 100)
            .await
            .unwrap();
        // NULLS LAST in both directions.
        let null_positions: Vec<usize> = page
            .iter()
            .enumerate()
            .filter(|(_, i)| i.chunk_count.is_none())
            .map(|(n, _)| n)
            .collect();
        assert_eq!(null_positions.len(), 1, "{}", sort.as_str());
        assert_eq!(
            null_positions[0],
            page.len() - 1,
            "{}: the uncounted document must sort last",
            sort.as_str()
        );
    }
}

#[tokio::test]
async fn a_document_on_two_connectors_is_counted_once_and_attributed_deterministically() {
    let Some(db) = common::seeded().await else {
        return common::skip(
            "a_document_on_two_connectors_is_counted_once_and_attributed_deterministically",
        );
    };
    let pool = db.pool.clone();

    let filter = DocumentFilter::default();
    let plan = documents::plan_connector_filter(&pool, &filter)
        .await
        .unwrap();
    let count = documents::count_documents(&pool, &filter, &plan)
        .await
        .unwrap();
    let items = list(&pool, &filter, SortOrder::UpdatedDesc, 100).await;
    assert_eq!(
        count as usize,
        items.len(),
        "count and listing disagree, which means the join is duplicating rows"
    );

    let shared: Vec<_> = items.iter().filter(|i| i.id == docs::SHARED).collect();
    assert_eq!(shared.len(), 1, "the shared document appeared twice");
    // Lowest connector id wins, every time.
    assert_eq!(shared[0].connector_id, Some(1));
    assert_eq!(shared[0].connector_name.as_deref(), Some("tildes-like"));
}

#[tokio::test]
async fn connector_and_source_filters_agree_between_both_query_shapes() {
    let Some(db) = common::seeded().await else {
        return common::skip("connector_and_source_filters_agree_between_both_query_shapes");
    };
    let pool = db.pool.clone();

    // Same predicate, forced down each plan: the results must be identical.
    for filter in [
        DocumentFilter {
            connector_id: Some(1),
            ..Default::default()
        },
        DocumentFilter {
            source: Some("web".into()),
            ..Default::default()
        },
        DocumentFilter {
            source: Some("GITHUB".into()),
            ..Default::default()
        },
    ] {
        let ids: Vec<i32> = sqlx::query_scalar(
            "SELECT c.id FROM public.connector c \
             WHERE ($1::int IS NULL OR c.id = $1) \
               AND ($2::text IS NULL OR upper(c.source) = upper($2))",
        )
        .bind(filter.connector_id)
        .bind(filter.source.as_deref())
        .fetch_all(&pool)
        .await
        .unwrap();

        let selective = documents::list_documents(
            &pool,
            &filter,
            &ConnectorPlan::Selective(ids.clone()),
            SortOrder::UpdatedDesc,
            Position::Offset(0),
            100,
        )
        .await
        .unwrap();
        let broad = documents::list_documents(
            &pool,
            &filter,
            &ConnectorPlan::Broad(ids),
            SortOrder::UpdatedDesc,
            Position::Offset(0),
            100,
        )
        .await
        .unwrap();

        assert_eq!(
            selective.iter().map(|i| &i.id).collect::<Vec<_>>(),
            broad.iter().map(|i| &i.id).collect::<Vec<_>>(),
            "the two plans disagree for {filter:?}"
        );
        assert!(!selective.is_empty(), "no rows matched {filter:?}");
    }
}

#[tokio::test]
async fn source_filtering_is_case_insensitive_and_an_unknown_source_matches_nothing() {
    let Some(db) = common::seeded().await else {
        return common::skip(
            "source_filtering_is_case_insensitive_and_an_unknown_source_matches_nothing",
        );
    };
    let pool = db.pool.clone();

    for spelling in ["github", "GITHUB", "GitHub"] {
        let items = list(
            &pool,
            &DocumentFilter {
                source: Some(spelling.into()),
                ..Default::default()
            },
            SortOrder::UpdatedDesc,
            20,
        )
        .await;
        assert_eq!(
            items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec![docs::GITHUB],
            "source={spelling}"
        );
    }

    let filter = DocumentFilter {
        source: Some("dropbox".into()),
        ..Default::default()
    };
    let plan = documents::plan_connector_filter(&pool, &filter)
        .await
        .unwrap();
    assert_eq!(
        plan,
        ConnectorPlan::Selective(Vec::new()),
        "a source with no connectors must resolve to an empty id set, not a table scan"
    );
    assert!(list(&pool, &filter, SortOrder::UpdatedDesc, 20)
        .await
        .is_empty());
}

#[tokio::test]
async fn search_matches_title_and_url_and_escapes_like_metacharacters() {
    let Some(db) = common::seeded().await else {
        return common::skip("search_matches_title_and_url_and_escapes_like_metacharacters");
    };
    let pool = db.pool.clone();

    let by_title = list(
        &pool,
        &DocumentFilter {
            search: Some("Newest".into()),
            ..Default::default()
        },
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    assert_eq!(
        by_title.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec![docs::NEWEST]
    );

    let by_url = list(
        &pool,
        &DocumentFilter {
            search: Some("github.com".into()),
            ..Default::default()
        },
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    assert_eq!(
        by_url.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec![docs::GITHUB]
    );

    // A bare `%` must mean a literal percent sign, not "match everything".
    let wildcard = list(
        &pool,
        &DocumentFilter {
            search: Some("%".into()),
            ..Default::default()
        },
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    assert!(
        wildcard.is_empty(),
        "'%' matched {} rows; LIKE metacharacters are not escaped",
        wildcard.len()
    );

    let underscore = list(
        &pool,
        &DocumentFilter {
            search: Some("_".into()),
            ..Default::default()
        },
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    assert!(
        underscore.is_empty(),
        "'_' matched as a single-char wildcard"
    );
}

#[tokio::test]
async fn hidden_filter_selects_both_ways() {
    let Some(db) = common::seeded().await else {
        return common::skip("hidden_filter_selects_both_ways");
    };
    let pool = db.pool.clone();

    let hidden = list(
        &pool,
        &DocumentFilter {
            hidden: Some(true),
            ..Default::default()
        },
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    assert_eq!(
        hidden.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec![docs::HIDDEN]
    );

    let visible = list(
        &pool,
        &DocumentFilter {
            hidden: Some(false),
            ..Default::default()
        },
        SortOrder::UpdatedDesc,
        20,
    )
    .await;
    assert!(!visible.iter().any(|i| i.id == docs::HIDDEN));
    assert!(visible.len() >= 9);
}

#[tokio::test]
async fn detail_reports_the_owning_cc_pair_and_recrawl_risk() {
    let Some(db) = common::seeded().await else {
        return common::skip("detail_reports_the_owning_cc_pair_and_recrawl_risk");
    };
    let pool = db.pool.clone();

    // Owned by an ACTIVE cc-pair, so a delete would likely be undone.
    let active = documents::get_document(&pool, docs::NEWEST)
        .await
        .unwrap()
        .expect("document exists");
    assert!(active.pg_row);
    assert_eq!(active.cc_pair_status.as_deref(), Some("ACTIVE"));
    assert!(active.recrawl_risk);
    assert_eq!(active.item.boost, 3);

    // Owned by a PAUSED cc-pair: no scheduled refresh, so no recrawl risk.
    let paused = documents::get_document(&pool, docs::TRICKY)
        .await
        .unwrap()
        .expect("document exists");
    assert_eq!(paused.cc_pair_status.as_deref(), Some("PAUSED"));
    assert!(!paused.recrawl_risk);

    assert!(documents::get_document(&pool, "https://example.com/nope")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn document_ids_with_urls_query_strings_and_unicode_round_trip() {
    let Some(db) = common::seeded().await else {
        return common::skip("document_ids_with_urls_query_strings_and_unicode_round_trip");
    };
    let pool = db.pool.clone();

    let detail = documents::get_document(&pool, docs::TRICKY)
        .await
        .unwrap()
        .expect("a document id containing ?, & and non-ASCII must be addressable");
    assert_eq!(detail.item.id, docs::TRICKY);
    assert_eq!(detail.item.semantic_id, "Tricky Id");
}

#[tokio::test]
async fn tags_are_returned_for_a_document_and_bounded() {
    let Some(db) = common::seeded().await else {
        return common::skip("tags_are_returned_for_a_document_and_bounded");
    };
    let pool = db.pool.clone();

    let tags = documents::get_document_tags(&pool, docs::DELETE_ME, 200)
        .await
        .unwrap();
    let pairs: Vec<(String, String)> = tags
        .iter()
        .map(|t| (t.key.clone(), t.value.clone()))
        .collect();
    assert!(pairs.contains(&("author".to_string(), "carol".to_string())));
    assert!(pairs.contains(&("topic".to_string(), "economics".to_string())));

    let bounded = documents::get_document_tags(&pool, docs::DELETE_ME, 1)
        .await
        .unwrap();
    assert_eq!(bounded.len(), 1, "the limit must be honoured");
}

#[tokio::test]
async fn deleting_a_tagged_document_succeeds_and_clears_every_fk_child() {
    let Some(db) = common::seeded().await else {
        return common::skip("deleting_a_tagged_document_succeeds_and_clears_every_fk_child");
    };
    let pool = db.pool.clone();

    // The C3 regression. The old sweep cleared only
    // document_by_connector_credential_pair, so deleting a *tagged* document
    // failed outright on the document__tag foreign key — and 444,793 tag links
    // exist in production.
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.document__tag WHERE document_id = $1")
            .bind(docs::DELETE_ME)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, 2, "fixture must give the target real tag links");

    documents::delete_document_pg_only(&pool, docs::DELETE_ME)
        .await
        .expect("deleting a tagged document must succeed");

    for (table, column) in [
        ("document", "id"),
        ("document__tag", "document_id"),
        ("chunk_stats", "document_id"),
        ("document_by_connector_credential_pair", "id"),
    ] {
        let remaining: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM public.{table} WHERE {column} = $1"
        )))
        .bind(docs::DELETE_ME)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            remaining, 0,
            "{table} still has rows for the deleted document"
        );
    }

    // Other documents' tags are untouched.
    let others: i64 = sqlx::query_scalar("SELECT count(*) FROM public.document__tag")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        others, 2,
        "the sweep deleted more than its own document's tags"
    );
}

#[tokio::test]
async fn deleting_a_missing_document_is_not_found_and_changes_nothing() {
    let Some(db) = common::seeded().await else {
        return common::skip("deleting_a_missing_document_is_not_found_and_changes_nothing");
    };
    let pool = db.pool.clone();

    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM public.document")
        .fetch_one(&pool)
        .await
        .unwrap();

    let err = documents::delete_document_pg_only(&pool, "https://example.com/never")
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::NotFound { .. }));

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM public.document")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn recrawl_risk_is_read_before_the_row_disappears() {
    let Some(db) = common::seeded().await else {
        return common::skip("recrawl_risk_is_read_before_the_row_disappears");
    };
    let pool = db.pool.clone();

    assert!(documents::recrawl_risk(&pool, docs::DELETE_ME)
        .await
        .unwrap());
    assert!(!documents::recrawl_risk(&pool, docs::TRICKY).await.unwrap());
    // A document that does not exist carries no risk.
    assert!(!documents::recrawl_risk(&pool, "https://example.com/never")
        .await
        .unwrap());
}

#[tokio::test]
async fn update_merges_metadata_and_never_touches_the_crawl_timestamp() {
    let Some(db) = common::seeded().await else {
        return common::skip("update_merges_metadata_and_never_touches_the_crawl_timestamp");
    };
    let pool = db.pool.clone();

    let before = documents::get_document(&pool, docs::OLDEST)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.item.metadata.as_ref().unwrap()["keep"], "me");

    let affected = documents::update_document(
        &pool,
        docs::OLDEST,
        &DocumentUpdate {
            semantic_id: Some("Renamed".into()),
            boost: Some(5),
            hidden: Some(true),
            metadata_merge: Some(serde_json::json!({ "author": "dave", "extra": 1 })),
        },
    )
    .await
    .unwrap();
    assert_eq!(affected, 1);

    let after = documents::get_document(&pool, docs::OLDEST)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.item.semantic_id, "Renamed");
    assert_eq!(after.item.boost, 5);
    assert!(after.item.hidden);

    let metadata = after.item.metadata.as_ref().unwrap();
    // Merged, not replaced: the old CLI edit wrote the whole object back and lost
    // every key it had not read.
    assert_eq!(metadata["keep"], "me", "an unrelated metadata key was lost");
    assert_eq!(
        metadata["author"], "dave",
        "the supplied key was not applied"
    );
    assert_eq!(metadata["extra"], 1);

    // doc_updated_at is Onyx's crawl timestamp; last_modified drives its sync
    // detection. Neither may move because OVIS renamed something.
    assert_eq!(after.item.doc_updated_at, before.item.doc_updated_at);
    assert_eq!(after.item.last_modified, before.item.last_modified);
    assert_eq!(after.last_synced, before.last_synced);
}

#[tokio::test]
async fn a_partial_update_leaves_untouched_fields_alone() {
    let Some(db) = common::seeded().await else {
        return common::skip("a_partial_update_leaves_untouched_fields_alone");
    };
    let pool = db.pool.clone();

    let before = documents::get_document(&pool, docs::NEWEST)
        .await
        .unwrap()
        .unwrap();
    documents::update_document(
        &pool,
        docs::NEWEST,
        &DocumentUpdate {
            hidden: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let after = documents::get_document(&pool, docs::NEWEST)
        .await
        .unwrap()
        .unwrap();
    assert!(after.item.hidden);
    assert_eq!(after.item.semantic_id, before.item.semantic_id);
    assert_eq!(after.item.boost, before.item.boost);
    assert_eq!(after.item.metadata, before.item.metadata);
}

#[tokio::test]
async fn updating_a_missing_document_affects_no_rows() {
    let Some(db) = common::seeded().await else {
        return common::skip("updating_a_missing_document_affects_no_rows");
    };
    let pool = db.pool.clone();

    let affected = documents::update_document(
        &pool,
        "https://example.com/never",
        &DocumentUpdate {
            semantic_id: Some("nope".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(affected, 0);
}

#[tokio::test]
async fn hydration_by_id_preserves_order_independence_and_filters_by_connector() {
    let Some(db) = common::seeded().await else {
        return common::skip(
            "hydration_by_id_preserves_order_independence_and_filters_by_connector",
        );
    };
    let pool = db.pool.clone();

    let ids = vec![
        docs::GITHUB.to_string(),
        docs::NEWEST.to_string(),
        "https://example.com/never".to_string(),
    ];
    let all = documents::documents_by_ids(&pool, &ids, None)
        .await
        .unwrap();
    assert_eq!(all.len(), 2, "a missing id must be omitted, not faked");

    // Connector 4 owns only the GitHub document.
    let filtered = documents::documents_by_ids(&pool, &ids, Some(&[4]))
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, docs::GITHUB);

    assert!(documents::documents_by_ids(&pool, &[], None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn counts_track_filters_and_the_estimate_is_positive() {
    let Some(db) = common::seeded().await else {
        return common::skip("counts_track_filters_and_the_estimate_is_positive");
    };
    let pool = db.pool.clone();

    let total = documents::count_documents(
        &pool,
        &DocumentFilter::default(),
        &ConnectorPlan::Unfiltered,
    )
    .await
    .unwrap();
    assert_eq!(total, 10);

    let hidden = documents::count_documents(
        &pool,
        &DocumentFilter {
            hidden: Some(true),
            ..Default::default()
        },
        &ConnectorPlan::Unfiltered,
    )
    .await
    .unwrap();
    assert_eq!(hidden, 1);

    // The estimate needs ANALYZE to be meaningful; assert only that it is sane.
    sqlx::query("ANALYZE public.document")
        .execute(&pool)
        .await
        .unwrap();
    let estimate = documents::estimate_total_documents(&pool).await.unwrap();
    assert!(estimate >= 0, "a negative reltuples must be clamped");
}

#[tokio::test]
async fn deep_offset_paging_is_refused_rather_than_served_slowly() {
    let Some(db) = common::seeded().await else {
        return common::skip("deep_offset_paging_is_refused_rather_than_served_slowly");
    };
    let pool = db.pool.clone();

    let err = documents::list_documents(
        &pool,
        &DocumentFilter::default(),
        &ConnectorPlan::Unfiltered,
        SortOrder::UpdatedDesc,
        Position::Offset(documents::MAX_OFFSET_DEPTH),
        50,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)));
    assert!(err.to_string().contains("next_cursor"));
}
