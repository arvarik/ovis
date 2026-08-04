//! Trash: capture → delete → restore, against the real Onyx schema.
//!
//! These are the tests that make aggressive pruning defensible. A document is
//! deleted through exactly the cascade production uses — every FK child, then
//! the row itself — and then brought back, and the assertions compare the
//! restored row against the original field by field rather than checking that
//! *something* came back.
//!
//! The OpenSearch side is a wiremock stand-in that records what was written,
//! so the chunk half of the round trip is verified too: the same `_id`s, the
//! same content, and vectors that survive the f16 packing intact.

mod common;

use ovis_core::db::trash::{self, TrashProvenance};
use ovis_core::search::OsClient;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const INDEX: &str = "danswer_chunk_test";

/// Deterministic 768-dim vector so the f16 round trip is checkable.
fn vector_for(chunk: usize) -> Vec<f32> {
    (0..768)
        .map(|i| ((i + chunk * 7) as f32 / 768.0) - 0.5)
        .collect()
}

fn chunk_source(doc_id: &str, index: usize) -> Value {
    json!({
        "document_id": doc_id,
        "chunk_index": index,
        "content": format!("chunk {index} of {doc_id} with enough words to be a real body"),
        "title": "Round Trip Fixture",
        "semantic_identifier": "Round Trip Fixture",
        "source_type": "web",
        "hidden": false,
        "boost": 0,
        "source_links": { "0": doc_id },
        "content_vector": vector_for(index),
        "title_vector": vector_for(99),
    })
}

/// OpenSearch stand-in that serves chunks and captures bulk writes.
async fn mock_os(doc_id: &str, chunk_count: usize) -> (MockServer, Arc<Mutex<Vec<String>>>) {
    let server = MockServer::start().await;
    let bulk_bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let owned_id = doc_id.to_string();
    Mock::given(method("POST"))
        .and(path_regex(r".*/_search$"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let requested = body["query"]["term"]["document_id"].as_str().unwrap_or("");
            // Paging: the second call (search_after) returns nothing.
            if requested != owned_id || body["search_after"][0].as_i64().is_some() {
                return ResponseTemplate::new(200).set_body_json(json!({
                    "hits": { "total": { "value": 0 }, "hits": [] }
                }));
            }
            let hits: Vec<Value> = (0..chunk_count)
                .map(|i| {
                    json!({
                        "_id": format!("{owned_id}__{i}"),
                        "_source": chunk_source(&owned_id, i),
                    })
                })
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({
                "hits": { "total": { "value": chunk_count }, "hits": hits }
            }))
        })
        .mount(&server)
        .await;

    let recorder = bulk_bodies.clone();
    Mock::given(method("POST"))
        .and(path_regex(r".*/_bulk.*"))
        .respond_with(move |request: &wiremock::Request| {
            let body = String::from_utf8_lossy(&request.body).to_string();
            let items = body.lines().filter(|l| l.contains("\"index\"")).count();
            recorder.lock().unwrap().push(body);
            ResponseTemplate::new(200).set_body_json(json!({
                "errors": false,
                "items": (0..items).map(|_| json!({"index": {"status": 201}})).collect::<Vec<_>>()
            }))
        })
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/_delete_by_query.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": chunk_count })))
        .mount(&server)
        .await;

    (server, bulk_bodies)
}

/// A document with tags and connector attribution, so the round trip has FK
/// children to lose.
async fn seed_document(pool: &PgPool, doc_id: &str) {
    sqlx::query(
        "INSERT INTO public.document \
             (id, boost, hidden, semantic_id, link, doc_updated_at, last_modified, chunk_count, \
              doc_metadata, from_ingestion_api, content_hash, primary_owners) \
         VALUES ($1, 7, false, 'Round Trip Fixture', $1, now() - interval '3 days', \
                 now() - interval '3 days', 3, '{\"kind\": \"fixture\"}'::jsonb, false, \
                 'trash-hash-1', ARRAY['owner@example.com'])",
    )
    .bind(doc_id)
    .execute(pool)
    .await
    .expect("seed document");

    sqlx::query(
        "INSERT INTO public.document_by_connector_credential_pair \
             (id, connector_id, credential_id, has_been_indexed) \
         VALUES ($1, 1, 1, true)",
    )
    .bind(doc_id)
    .execute(pool)
    .await
    .expect("seed cc pair");

    // Tag values are unique per (key, value, source), so each document needs
    // its own or a test that seeds several collides with itself.
    let tag_id: i32 = sqlx::query_scalar(
        "INSERT INTO public.tag (tag_key, tag_value, source) VALUES ('topic', $1, 'web') \
         RETURNING id",
    )
    .bind(format!("trash-test-{doc_id}"))
    .fetch_one(pool)
    .await
    .expect("seed tag");
    sqlx::query("INSERT INTO public.document__tag (document_id, tag_id) VALUES ($1, $2)")
        .bind(doc_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .expect("seed tag link");
}

async fn document_json(pool: &PgPool, doc_id: &str) -> Option<Value> {
    let row = sqlx::query("SELECT row_to_json(d) AS j FROM public.document d WHERE d.id = $1")
        .bind(doc_id)
        .fetch_optional(pool)
        .await
        .expect("read document");
    row.map(|r| r.get::<Value, _>("j"))
}

#[tokio::test]
async fn a_trashed_document_disappears_from_onyx_and_comes_back_identical() {
    let Some(db) = common::seeded().await else {
        return common::skip("a_trashed_document_disappears_from_onyx_and_comes_back_identical");
    };
    let doc_id = "https://example.com/trash-round-trip";
    seed_document(&db, doc_id).await;
    assert!(trash::ensure_tables(&db).await, "trash DDL must apply");

    let (os_server, bulk_bodies) = mock_os(doc_id, 3).await;
    let os = OsClient::new(&os_server.uri(), None, None).unwrap();

    let before = document_json(&db, doc_id).await.expect("seeded document");

    // ---- capture + delete ----
    let snapshot = trash::capture(&db, &os, INDEX, doc_id, true)
        .await
        .expect("capture");
    assert_eq!(snapshot.chunk_count(), 3, "every chunk must be captured");
    assert!(snapshot.vectors_included);
    assert_eq!(snapshot.tags.len(), 1, "tags must be captured");
    assert_eq!(snapshot.cc_pairs.len(), 1, "connector attribution must be captured");

    trash::trash_and_delete(
        &db,
        &snapshot,
        &TrashProvenance {
            deleted_by: "test".into(),
            ..Default::default()
        },
        30,
    )
    .await
    .expect("trash and delete");

    // Onyx can no longer see it, anywhere.
    assert!(
        document_json(&db, doc_id).await.is_none(),
        "the document row must be gone from Onyx"
    );
    let orphan_tags: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.document__tag WHERE document_id = $1")
            .bind(doc_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(orphan_tags, 0, "tag links must be gone");
    let orphan_pairs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.document_by_connector_credential_pair WHERE id = $1",
    )
    .bind(doc_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(orphan_pairs, 0, "connector attribution must be gone");

    // But OVIS still has it.
    let (items, total) = trash::list(&db, &Default::default(), 10, 0).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(items[0].document_id, doc_id);
    assert_eq!(items[0].chunk_count, 3);
    assert!(items[0].snapshot_bytes > 0);
    assert!(!items[0].reappeared);

    // ---- restore ----
    let outcome = trash::restore(&db, &os, INDEX, doc_id, false)
        .await
        .expect("restore");
    assert_eq!(outcome.chunks_restored, 3);
    assert_eq!(outcome.tags_restored, 1);
    assert_eq!(outcome.cc_pairs_restored, 1);
    assert_eq!(outcome.skipped_tags, 0);
    assert_eq!(outcome.skipped_cc_pairs, 0);
    assert!(!outcome.index_restore_pending);

    let after = document_json(&db, doc_id).await.expect("restored document");
    let (before_obj, after_obj) = (before.as_object().unwrap(), after.as_object().unwrap());
    for (column, original) in before_obj {
        assert_eq!(
            after_obj.get(column),
            Some(original),
            "column `{column}` did not round-trip"
        );
    }

    let tag_links: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.document__tag WHERE document_id = $1")
            .bind(doc_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(tag_links, 1, "the tag link must come back");

    // The chunks were written back under their original ids, with vectors.
    let bulk = bulk_bodies.lock().unwrap().join("\n");
    for i in 0..3 {
        assert!(
            bulk.contains(&format!("{doc_id}__{i}")),
            "chunk {i} must be re-indexed under its original id"
        );
    }
    assert!(
        bulk.contains("content_vector"),
        "vectors must be restored so the document is semantically searchable at once"
    );
    assert!(
        !bulk.contains("__f16_b64"),
        "the packed form is a storage detail and must be expanded before indexing"
    );

    // The restored vector must still be usable, not mangled by f16 packing.
    let indexed: Value = bulk
        .lines()
        .filter(|line| line.contains("content_vector"))
        .find_map(|line| serde_json::from_str(line).ok())
        .expect("a chunk body was indexed");
    let restored: Vec<f64> = indexed["content_vector"]
        .as_array()
        .expect("vector array")
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(restored.len(), 768);
    let expected = vector_for(indexed["chunk_index"].as_u64().unwrap() as usize);
    for (i, value) in restored.iter().enumerate() {
        assert!(
            (*value as f32 - expected[i]).abs() < 0.001,
            "vector component {i}: {value} vs {}",
            expected[i]
        );
    }

    // Restoring twice is refused rather than silently duplicating.
    let err = trash::restore(&db, &os, INDEX, doc_id, false)
        .await
        .expect_err("a restored snapshot is spent");
    assert!(err.to_string().contains("trashed document"), "{err}");
}

#[tokio::test]
async fn a_failed_cascade_leaves_no_snapshot_and_no_deletion() {
    let Some(db) = common::seeded().await else {
        return common::skip("a_failed_cascade_leaves_no_snapshot_and_no_deletion");
    };
    let doc_id = "https://example.com/trash-atomicity";
    seed_document(&db, doc_id).await;
    assert!(trash::ensure_tables(&db).await);

    let (os_server, _) = mock_os(doc_id, 1).await;
    let os = OsClient::new(&os_server.uri(), None, None).unwrap();
    let snapshot = trash::capture(&db, &os, INDEX, doc_id, true)
        .await
        .expect("capture");

    // Delete the row underneath the snapshot, so the cascade inside
    // `trash_and_delete` finds nothing and must roll the snapshot back with it.
    sqlx::query("DELETE FROM public.document__tag WHERE document_id = $1")
        .bind(doc_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM public.document_by_connector_credential_pair WHERE id = $1")
        .bind(doc_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM public.document WHERE id = $1")
        .bind(doc_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let result = trash::trash_and_delete(
        &db,
        &snapshot,
        &TrashProvenance {
            deleted_by: "test".into(),
            ..Default::default()
        },
        30,
    )
    .await;
    assert!(result.is_err(), "deleting a vanished document must fail");

    let (_, total) = trash::list(&db, &Default::default(), 10, 0).await.unwrap();
    assert_eq!(
        total, 0,
        "a rolled-back deletion must leave no snapshot behind — otherwise the trash \
         accumulates entries for documents that were never deleted"
    );
}

#[tokio::test]
async fn restoring_over_a_recrawled_document_is_refused_unless_asked() {
    let Some(db) = common::seeded().await else {
        return common::skip("restoring_over_a_recrawled_document_is_refused_unless_asked");
    };
    let doc_id = "https://example.com/trash-recrawl";
    seed_document(&db, doc_id).await;
    assert!(trash::ensure_tables(&db).await);

    let (os_server, _) = mock_os(doc_id, 2).await;
    let os = OsClient::new(&os_server.uri(), None, None).unwrap();
    let snapshot = trash::capture(&db, &os, INDEX, doc_id, true).await.unwrap();
    trash::trash_and_delete(&db, &snapshot, &TrashProvenance::default(), 30)
        .await
        .unwrap();

    // The crawler brings it back with different content.
    sqlx::query(
        "INSERT INTO public.document (id, boost, hidden, semantic_id, link, last_modified, \
             chunk_count, doc_metadata, from_ingestion_api) \
         VALUES ($1, 0, false, 'Recrawled', $1, now(), 5, '{}'::jsonb, false)",
    )
    .bind(doc_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let (items, _) = trash::list(&db, &Default::default(), 10, 0).await.unwrap();
    assert!(
        items[0].reappeared,
        "the trash listing must warn that the id is back in Onyx"
    );

    let err = trash::restore(&db, &os, INDEX, doc_id, false)
        .await
        .expect_err("restoring over a live document must be refused");
    assert!(err.to_string().contains("already exists"), "{err}");

    let semantic: String =
        sqlx::query_scalar("SELECT semantic_id FROM public.document WHERE id = $1")
            .bind(doc_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(semantic, "Recrawled", "the live document must be untouched");

    // With overwrite, the snapshot wins.
    let outcome = trash::restore(&db, &os, INDEX, doc_id, true)
        .await
        .expect("overwrite restore");
    assert_eq!(outcome.chunks_restored, 2);
    let semantic: String =
        sqlx::query_scalar("SELECT semantic_id FROM public.document WHERE id = $1")
            .bind(doc_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(semantic, "Round Trip Fixture");
}

#[tokio::test]
async fn retention_purges_only_expired_unheld_snapshots() {
    let Some(db) = common::seeded().await else {
        return common::skip("retention_purges_only_expired_unheld_snapshots");
    };
    assert!(trash::ensure_tables(&db).await);
    let (os_server, _) = mock_os("", 0).await;
    let os = OsClient::new(&os_server.uri(), None, None).unwrap();

    for (suffix, retention) in [("expired", 1), ("fresh", 30), ("held", 1)] {
        let doc_id = format!("https://example.com/trash-{suffix}");
        seed_document(&db, &doc_id).await;
        let snapshot = trash::capture(&db, &os, INDEX, &doc_id, false).await.unwrap();
        trash::trash_and_delete(&db, &snapshot, &TrashProvenance::default(), retention)
            .await
            .unwrap();
    }
    // Age the two one-day snapshots past their retention.
    sqlx::query(
        "UPDATE ovis.trash_document SET expires_at = now() - interval '1 day' \
         WHERE document_id LIKE '%trash-expired' OR document_id LIKE '%trash-held'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(trash::set_hold(&db, "https://example.com/trash-held", true)
        .await
        .unwrap());

    let due = trash::due_for_purge(&db, 100).await.unwrap();
    assert_eq!(
        due,
        vec!["https://example.com/trash-expired".to_string()],
        "only the expired, unheld snapshot is due"
    );

    let counts = trash::counts(&db).await.unwrap();
    assert_eq!(counts.items, 3);
    assert_eq!(counts.on_hold, 1);
    assert!(counts.bytes > 0);

    assert_eq!(trash::purge(&db, &due).await.unwrap(), 1);
    let (_, total) = trash::list(&db, &Default::default(), 10, 0).await.unwrap();
    assert_eq!(total, 2, "the held and fresh snapshots survive the purge");
}

#[tokio::test]
async fn a_snapshot_from_a_newer_build_is_refused_rather_than_half_restored() {
    let Some(db) = common::seeded().await else {
        return common::skip("a_snapshot_from_a_newer_build_is_refused_rather_than_half_restored");
    };
    let doc_id = "https://example.com/trash-version";
    seed_document(&db, doc_id).await;
    assert!(trash::ensure_tables(&db).await);
    let (os_server, _) = mock_os(doc_id, 1).await;
    let os = OsClient::new(&os_server.uri(), None, None).unwrap();

    let snapshot = trash::capture(&db, &os, INDEX, doc_id, true).await.unwrap();
    trash::trash_and_delete(&db, &snapshot, &TrashProvenance::default(), 30)
        .await
        .unwrap();
    sqlx::query("UPDATE ovis.trash_document SET snapshot_version = 99 WHERE document_id = $1")
        .bind(doc_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let err = trash::restore(&db, &os, INDEX, doc_id, false)
        .await
        .expect_err("a future snapshot must not be restored on a guess");
    assert!(err.to_string().contains("version 99"), "{err}");
    assert!(
        document_json(&db, doc_id).await.is_none(),
        "nothing may be partially written"
    );
}
