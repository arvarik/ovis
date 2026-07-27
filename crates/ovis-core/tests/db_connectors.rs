//! Integration tests for connector summaries, indexing telemetry, tags and
//! stats, against the real Onyx DDL.

mod common;

use ovis_core::db::{connectors, indexing, probe, stats, tags};

#[tokio::test]
async fn summaries_report_the_real_cc_pair_status() {
    let Some(db) = common::seeded().await else {
        return common::skip("summaries_report_the_real_cc_pair_status");
    };

    let summaries = connectors::list_summaries(&db.pool).await.unwrap();
    assert_eq!(summaries.len(), 4);

    let by_name = |name: &str| {
        summaries
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no summary named {name}"))
    };

    // The C5 regression: the old query selected the literal `false AS disabled`
    // and never read cc_pair.status, so all 278 PAUSED connectors in production
    // displayed as healthy.
    assert_eq!(by_name("tildes-like").status, "ACTIVE");
    assert_eq!(by_name("paused-web").status, "PAUSED");
    assert_eq!(by_name("code-mirror").status, "INITIAL_INDEXING");
    assert!(by_name("parked-web").in_repeated_error_state);
}

#[tokio::test]
async fn summaries_count_documents_from_dcc_not_from_total_docs_indexed() {
    let Some(db) = common::seeded().await else {
        return common::skip("summaries_count_documents_from_dcc_not_from_total_docs_indexed");
    };

    let summaries = connectors::list_summaries(&db.pool).await.unwrap();
    let paused = summaries.iter().find(|s| s.name == "paused-web").unwrap();

    // The fixture sets this cc-pair's `total_docs_indexed` to 99999 on purpose:
    // the column is unreliable in production and must never be read.
    assert_eq!(
        paused.doc_count, 2,
        "doc_count came from total_docs_indexed instead of dcc"
    );

    let active = summaries.iter().find(|s| s.name == "tildes-like").unwrap();
    // 8 dcc rows for connector 1, and `total_docs_indexed` says 0.
    assert_eq!(active.doc_count, 8);

    // Ordered biggest-first.
    let counts: Vec<i64> = summaries.iter().map(|s| s.doc_count).collect();
    let mut sorted = counts.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(counts, sorted);
}

#[tokio::test]
async fn a_park_sentinel_marks_the_pair_parked_and_is_passed_through_verbatim() {
    let Some(db) = common::seeded().await else {
        return common::skip(
            "a_park_sentinel_marks_the_pair_parked_and_is_passed_through_verbatim",
        );
    };

    let summaries = connectors::list_summaries(&db.pool).await.unwrap();
    let parked = summaries.iter().find(|s| s.name == "parked-web").unwrap();
    assert!(
        parked.parked,
        "the resilience-cron sentinel was not detected"
    );
    assert_eq!(
        parked.last_attempt.as_ref().unwrap().error_msg.as_deref(),
        Some("first-pass already complete"),
        "the sentinel must be surfaced verbatim, never rewritten"
    );

    // A pair whose latest failure is an ordinary error is not parked.
    let ordinary = summaries.iter().find(|s| s.name == "paused-web").unwrap();
    assert!(!ordinary.parked);
    assert_eq!(
        ordinary.last_attempt.as_ref().unwrap().error_msg.as_deref(),
        Some("connection reset by peer")
    );

    // The action guard reads the same signal.
    let reference = connectors::get_cc_pair_ref(&db.pool, 3).await.unwrap();
    assert!(reference.parked);
    assert_eq!(reference.connector_id, 3);
    assert_eq!(reference.credential_id, 1);
    assert_eq!(reference.name, "parked-web");
}

#[tokio::test]
async fn latest_attempt_is_the_latest_not_an_arbitrary_one() {
    let Some(db) = common::seeded().await else {
        return common::skip("latest_attempt_is_the_latest_not_an_arbitrary_one");
    };

    let summaries = connectors::list_summaries(&db.pool).await.unwrap();
    let active = summaries.iter().find(|s| s.name == "tildes-like").unwrap();
    // cc-pair 1 has attempts 1 (1 minute ago), 3 (1 day) and 6 (4 days).
    let last = active.last_attempt.as_ref().unwrap();
    assert_eq!(last.id, Some(1));
    assert_eq!(last.status.as_deref(), Some("IN_PROGRESS"));
}

#[tokio::test]
async fn cc_pair_detail_exposes_config_but_never_credential_secrets() {
    let Some(db) = common::seeded().await else {
        return common::skip("cc_pair_detail_exposes_config_but_never_credential_secrets");
    };

    let detail = connectors::get_detail(&db.pool, 1).await.unwrap().unwrap();
    assert_eq!(detail.summary.name, "tildes-like");
    let config = detail.connector_specific_config.as_ref().unwrap();
    assert_eq!(config["base_url"], "https://example.com/");
    assert_eq!(detail.credential_id, Some(1));
    assert_eq!(detail.credential_name.as_deref(), Some("web-credential"));
    assert_eq!(detail.summary.refresh_freq_secs, Some(2_592_000));

    let rendered = serde_json::to_string(&detail).unwrap();
    assert!(
        !rendered.contains("credential_json"),
        "the encrypted credential blob must never be read, let alone serialised"
    );

    assert!(connectors::get_detail(&db.pool, 9999)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn attempt_aggregates_are_scoped_and_global() {
    let Some(db) = common::seeded().await else {
        return common::skip("attempt_aggregates_are_scoped_and_global");
    };

    let global = connectors::attempt_aggregates(&db.pool, None)
        .await
        .unwrap();
    assert_eq!(global.in_progress, 2);
    assert_eq!(global.success, 1);
    assert_eq!(global.failed, 2);
    assert_eq!(global.canceled, 1);

    let scoped = connectors::attempt_aggregates(&db.pool, Some(1))
        .await
        .unwrap();
    assert_eq!(scoped.in_progress, 1);
    assert_eq!(scoped.success, 1);
    assert_eq!(scoped.canceled, 1);
    assert_eq!(scoped.failed, 0);
}

#[tokio::test]
async fn status_counts_match_the_seeded_state_including_parked() {
    let Some(db) = common::seeded().await else {
        return common::skip("status_counts_match_the_seeded_state_including_parked");
    };

    let counts = connectors::status_counts(&db.pool).await.unwrap();
    assert_eq!(counts.total, 4);
    assert_eq!(counts.active, 1);
    assert_eq!(counts.paused, 2);
    assert_eq!(counts.initial_indexing, 1);
    // Parked is a property of the latest attempt's message, not of status.
    assert_eq!(counts.parked, 1);
}

#[tokio::test]
async fn stalled_detection_uses_heartbeat_staleness_not_document_counts() {
    let Some(db) = common::seeded().await else {
        return common::skip("stalled_detection_uses_heartbeat_staleness_not_document_counts");
    };

    let attempts = indexing::list_attempts(&db.pool, None, None, 50, 0)
        .await
        .unwrap();
    let by_id = |id: i32| attempts.iter().find(|a| a.id == id).unwrap();

    // Attempt 1: running, heartbeat a minute ago, *zero* documents indexed.
    // Counting documents would flag this working crawl as dead.
    let fresh = by_id(1);
    assert_eq!(fresh.status, "IN_PROGRESS");
    assert_eq!(fresh.new_docs_indexed, Some(0));
    assert!(!fresh.stalled, "a fresh zero-document crawl is not stalled");

    // Attempt 2: running, no heartbeat for hours.
    assert!(by_id(2).stalled);
    assert!(by_id(2).pages_per_min.is_some());

    // Finished attempts are never stalled, however old.
    assert!(!by_id(3).stalled);
    assert!(!by_id(4).stalled);
    assert!(by_id(3).pages_per_min.is_none());

    let (running, stalled) = indexing::in_progress_counts(&db.pool).await.unwrap();
    assert_eq!(running, 2);
    assert_eq!(stalled, 1);
}

#[tokio::test]
async fn attempts_are_filterable_by_pair_and_status_and_counted_consistently() {
    let Some(db) = common::seeded().await else {
        return common::skip("attempts_are_filterable_by_pair_and_status_and_counted_consistently");
    };

    let all = indexing::list_attempts(&db.pool, None, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(
        all.len() as i64,
        indexing::count_attempts(&db.pool, None, None)
            .await
            .unwrap()
    );
    // Newest first.
    let times: Vec<_> = all.iter().map(|a| a.time_updated).collect();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(times, sorted);

    let statuses = vec!["in_progress".to_string()];
    let running = indexing::list_attempts(&db.pool, None, Some(&statuses), 50, 0)
        .await
        .unwrap();
    assert_eq!(running.len(), 2, "status matching must be case-insensitive");
    assert_eq!(
        indexing::count_attempts(&db.pool, None, Some(&statuses))
            .await
            .unwrap(),
        2
    );

    let scoped = indexing::list_attempts(&db.pool, Some(1), None, 50, 0)
        .await
        .unwrap();
    assert_eq!(scoped.len(), 3);
    assert!(scoped.iter().all(|a| a.cc_pair_id == 1));
    assert!(scoped
        .iter()
        .all(|a| a.connector_name.as_deref() == Some("tildes-like")));

    let one = indexing::get_attempt(&db.pool, 1).await.unwrap().unwrap();
    assert_eq!(one.id, 1);
    assert!(indexing::get_attempt(&db.pool, 9999)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn per_document_errors_are_listed_with_an_unresolved_filter() {
    let Some(db) = common::seeded().await else {
        return common::skip("per_document_errors_are_listed_with_an_unresolved_filter");
    };

    let all = indexing::list_attempt_errors(&db.pool, None, false, 50, 0)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(
        indexing::count_attempt_errors(&db.pool, None, false)
            .await
            .unwrap(),
        2
    );

    let unresolved = indexing::list_attempt_errors(&db.pool, None, true, 50, 0)
        .await
        .unwrap();
    assert_eq!(unresolved.len(), 1);
    assert!(!unresolved[0].is_resolved);
    assert_eq!(
        unresolved[0].document_id.as_deref(),
        Some("https://paused.example/broken")
    );

    let scoped = indexing::list_attempt_errors(&db.pool, Some(2), false, 50, 0)
        .await
        .unwrap();
    assert_eq!(scoped.len(), 2);
    assert!(
        indexing::list_attempt_errors(&db.pool, Some(1), false, 50, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn worker_level_errors_are_readable() {
    let Some(db) = common::seeded().await else {
        return common::skip("worker_level_errors_are_readable");
    };

    let all = indexing::list_background_errors(&db.pool, None, 50)
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].message.contains("celery"));
    assert_eq!(all[0].cc_pair_id, Some(2));
    assert!(indexing::list_background_errors(&db.pool, Some(1), 50)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn cc_pair_document_listing_is_authoritative_and_paged() {
    let Some(db) = common::seeded().await else {
        return common::skip("cc_pair_document_listing_is_authoritative_and_paged");
    };

    let total = connectors::count_docs(&db.pool, 1).await.unwrap();
    assert_eq!(total, 8);

    let first = connectors::list_docs(&db.pool, 1, 3, 0).await.unwrap();
    let second = connectors::list_docs(&db.pool, 1, 3, 3).await.unwrap();
    assert_eq!(first.len(), 3);
    assert_eq!(second.len(), 3);
    let overlap: std::collections::HashSet<&String> = first
        .iter()
        .map(|d| &d.id)
        .filter(|id| second.iter().any(|d| &d.id == *id))
        .collect();
    assert!(overlap.is_empty(), "offset paging repeated a row");

    // Newest first, like the main listing.
    let times: Vec<_> = first.iter().map(|d| d.updated_at).collect();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(times, sorted);
}

#[tokio::test]
async fn connector_history_sums_documents_added_per_day() {
    let Some(db) = common::seeded().await else {
        return common::skip("connector_history_sums_documents_added_per_day");
    };

    let history = connectors::history(&db.pool, 1, 7).await.unwrap();
    // Attempts 1, 3 and 6 fall inside 7 days for cc-pair 1.
    assert!(!history.is_empty());
    assert!(history.iter().any(|p| p.docs_added == 50));
    // Ascending by day, so a chart can render it directly.
    let days: Vec<&String> = history.iter().map(|p| &p.day).collect();
    let mut sorted = days.clone();
    sorted.sort();
    assert_eq!(days, sorted);
}

#[tokio::test]
async fn top_connectors_can_be_ranked_by_documents_or_recency() {
    let Some(db) = common::seeded().await else {
        return common::skip("top_connectors_can_be_ranked_by_documents_or_recency");
    };

    let by_docs = connectors::top_connectors(&db.pool, false, 10)
        .await
        .unwrap();
    assert_eq!(by_docs.first().unwrap().name, "tildes-like");
    let counts: Vec<i64> = by_docs.iter().map(|c| c.doc_count).collect();
    let mut sorted = counts.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(counts, sorted);

    let by_recent = connectors::top_connectors(&db.pool, true, 10)
        .await
        .unwrap();
    // The most recently indexed pair leads, and the never-indexed ones sort last.
    assert_eq!(by_recent.first().unwrap().name, "tildes-like");
    assert!(by_recent
        .last()
        .unwrap()
        .last_successful_index_time
        .is_none());

    assert_eq!(
        connectors::top_connectors(&db.pool, false, 2)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn tag_facets_are_counted_and_filterable() {
    let Some(db) = common::seeded().await else {
        return common::skip("tag_facets_are_counted_and_filterable");
    };

    let facets = tags::list_facets(&db.pool, None, None, 50).await.unwrap();
    let alice = facets
        .iter()
        .find(|f| f.key == "author" && f.value == "alice")
        .unwrap();
    assert_eq!(alice.doc_count, 2);

    // Most-used first.
    let counts: Vec<i64> = facets.iter().map(|f| f.doc_count).collect();
    let mut sorted = counts.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(counts, sorted);

    let authors = tags::list_facets(&db.pool, Some("author"), None, 50)
        .await
        .unwrap();
    assert!(authors.iter().all(|f| f.key == "author"));
    assert_eq!(authors.len(), 2);

    let prefixed = tags::list_facets(&db.pool, Some("author"), Some("al"), 50)
        .await
        .unwrap();
    assert_eq!(prefixed.len(), 1);
    assert_eq!(prefixed[0].value, "alice");

    let keys = tags::list_keys(&db.pool, 50).await.unwrap();
    let key_names: Vec<&String> = keys.iter().map(|(k, _)| k).collect();
    assert!(key_names.contains(&&"author".to_string()));
    assert!(key_names.contains(&&"topic".to_string()));
}

#[tokio::test]
async fn crawl_rate_and_timeline_read_last_modified() {
    let Some(db) = common::seeded().await else {
        return common::skip("crawl_rate_and_timeline_read_last_modified");
    };

    // Nothing in the fixture was touched in the last 15 minutes.
    assert_eq!(stats::docs_since(&db.pool, 15).await.unwrap(), 0);
    // Everything falls inside 60 days.
    assert_eq!(stats::docs_since(&db.pool, 60 * 24 * 60).await.unwrap(), 10);

    let timeline = stats::timeline(
        &db.pool,
        stats::TimelineWindow::Month,
        stats::TimelineBucketSize::Day,
    )
    .await
    .unwrap();
    // Empty buckets are filled with zero so a chart has no gaps.
    assert!(timeline.len() >= 30, "got {} buckets", timeline.len());
    assert!(timeline.iter().any(|b| b.docs > 0));
    assert!(timeline.iter().any(|b| b.docs == 0));
    let buckets: Vec<_> = timeline.iter().map(|b| b.bucket).collect();
    let mut sorted = buckets.clone();
    sorted.sort();
    assert_eq!(buckets, sorted, "buckets must be chronological");
}

#[tokio::test]
async fn per_source_stats_count_each_document_once() {
    let Some(db) = common::seeded().await else {
        return common::skip("per_source_stats_count_each_document_once");
    };

    let sources = stats::by_source(&db.pool).await.unwrap();
    let web = sources.iter().find(|s| s.source == "WEB").unwrap();
    let github = sources.iter().find(|s| s.source == "GITHUB").unwrap();

    // 9 WEB documents, one of which is attached to two WEB connectors — so a
    // naive count(*) would report 10.
    assert_eq!(
        web.documents, 9,
        "a multi-connector document was double-counted"
    );
    assert_eq!(web.connectors, 3);
    assert_eq!(github.documents, 1);
    assert_eq!(github.connectors, 1);
    // Chunk counts come from OpenSearch, not from here.
    assert!(web.chunks.is_none());
}

#[tokio::test]
async fn the_schema_probe_passes_against_the_captured_schema() {
    let Some(db) = common::seeded().await else {
        return common::skip("the_schema_probe_passes_against_the_captured_schema");
    };

    let result = probe::probe_schema(&db.pool).await.unwrap();
    assert!(
        result.missing_columns.is_empty(),
        "the probe wants columns the real schema does not have: {:?}",
        result.missing_columns
    );
    assert!(
        result.unhandled_fk_children.is_empty(),
        "the delete sweep does not cover: {:?}",
        result.unhandled_fk_children
    );
    // scripts/test-db.sh applies ops/onyx_indexes.sql, so all of them are present.
    assert!(
        result.missing_indexes.is_empty(),
        "missing support indexes: {:?}",
        result.missing_indexes
    );
    assert!(result.is_ok());
}

#[tokio::test]
async fn the_fk_probe_notices_a_new_restricting_child_table() {
    let Some(db) = common::seeded().await else {
        return common::skip("the_fk_probe_notices_a_new_restricting_child_table");
    };

    // Simulate an Onyx upgrade adding a table the delete sweep does not know
    // about. Delete must then refuse rather than fail mid-transaction.
    sqlx::query(
        "CREATE TABLE public.ovis_probe_child ( \
             document_id varchar NOT NULL REFERENCES public.document(id), \
             note text \
         )",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let unhandled = probe::probe_document_fk_children(&db.pool).await.unwrap();
    assert!(
        unhandled.contains(&"ovis_probe_child.document_id".to_string()),
        "a new restricting foreign key went unnoticed: {unhandled:?}"
    );

    sqlx::query("DROP TABLE public.ovis_probe_child")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(probe::probe_document_fk_children(&db.pool)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn search_settings_resolves_the_present_row_not_a_past_one() {
    let Some(db) = common::seeded().await else {
        return common::skip("search_settings_resolves_the_present_row_not_a_past_one");
    };

    let settings = probe::load_search_settings(&db.pool).await.unwrap();
    // The fixture holds a PAST row named `danswer_chunk` alongside the PRESENT
    // one. Picking the wrong one — or using a `danswer_chunk*` wildcard — is how a
    // delete fans out across a secondary index during a re-embed.
    assert_eq!(
        settings.index_name,
        "danswer_chunk_snowflake_arctic_embed_m"
    );
    assert_eq!(settings.model_name, "snowflake-arctic-embed:m");
    assert_eq!(settings.model_dim, 768);
    assert!(!settings.index_name.contains('*'));
}

#[tokio::test]
async fn missing_search_settings_is_a_schema_mismatch_not_a_guess() {
    let Some(db) = common::seeded().await else {
        return common::skip("missing_search_settings_is_a_schema_mismatch_not_a_guess");
    };

    sqlx::query("UPDATE public.search_settings SET status = 'PAST'")
        .execute(&db.pool)
        .await
        .unwrap();

    let err = probe::load_search_settings(&db.pool).await.unwrap_err();
    assert!(matches!(err, ovis_core::CoreError::SchemaMismatch(_)));
    assert!(err.to_string().contains("PRESENT"));
}

#[tokio::test]
async fn the_pending_index_delete_queue_is_created_and_drains_its_own_schema() {
    let Some(db) = common::seeded().await else {
        return common::skip("the_pending_index_delete_queue_is_created_and_drains_its_own_schema");
    };

    assert!(
        ovis_core::db::pending_deletes::ensure_table(&db.pool).await,
        "OVIS must be able to create its own schema"
    );
    assert_eq!(
        ovis_core::db::pending_deletes::pending_count(&db.pool)
            .await
            .unwrap(),
        0
    );

    ovis_core::db::pending_deletes::enqueue(&db.pool, "https://example.com/orphan", "boom")
        .await
        .unwrap();
    assert_eq!(
        ovis_core::db::pending_deletes::pending_count(&db.pool)
            .await
            .unwrap(),
        1
    );

    // Enqueueing twice must not duplicate the row.
    ovis_core::db::pending_deletes::enqueue(&db.pool, "https://example.com/orphan", "boom again")
        .await
        .unwrap();
    assert_eq!(
        ovis_core::db::pending_deletes::pending_count(&db.pool)
            .await
            .unwrap(),
        1
    );

    // Nothing OVIS owns lives in Onyx's schema.
    let in_public: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name LIKE 'ovis%'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(in_public, 0, "OVIS created a table in Onyx's schema");

    sqlx::query("DROP SCHEMA IF EXISTS ovis CASCADE")
        .execute(&db.pool)
        .await
        .unwrap();
}
