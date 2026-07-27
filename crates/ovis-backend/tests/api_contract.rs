//! HTTP-level contract tests: the whole router, driven with `tower::oneshot`.
//!
//! The point of view here is a client's. Every assertion is about what comes back
//! over the wire — status code, error `code`, envelope shape, headers, SSE frames
//! — because that is the contract the UI and CLI are written against.
//!
//! This is also where the old suite was most misleading: its 11 tests pointed at a
//! nonexistent database and asserted that a failure produced `200 OK` with an
//! empty list. These assert the inverse.
//!
//! Needs a seeded Postgres (`scripts/test-db.sh up`) and an OpenSearch stand-in,
//! which `wiremock` provides. Skips itself when `OVIS_TEST_DATABASE_URL` is unset.

use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use ovis_backend::config::ServerConfig;
use ovis_backend::state::{AppState, BuildInfo, Caches, RuntimeMeta};
use ovis_core::db::probe::SchemaProbe;
use ovis_core::search::{IndexCapabilities, OsClient};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, Request as MockRequest, ResponseTemplate};

const INDEX: &str = "danswer_chunk_snowflake_arctic_embed_m";

/// Serialise the tests: they share one database and each re-seeds it.
static EXCLUSIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Harness {
    app: axum::Router,
    state: AppState,
    /// The OpenSearch stand-in, kept alive for the test's duration.
    _os: MockServer,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

/// Stand in for OpenSearch: enough of the API that the routes work.
async fn mock_opensearch() -> MockServer {
    let server = MockServer::start().await;

    // Chunk fetches, searches and capability probes all POST to _search.
    Mock::given(method("POST"))
        .and(path_regex(r".*/_search$"))
        .respond_with(|request: &MockRequest| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);

            // Capability probe: `exists` on a vector field.
            if let Some(field) = body["query"]["exists"]["field"].as_str() {
                let count = if field == "content_vector" { 1 } else { 0 };
                return ResponseTemplate::new(200).set_body_json(json!({
                    "took": 1,
                    "hits": { "total": { "value": count, "relation": "eq" }, "hits": [] }
                }));
            }

            // Chunk fetch for one document.
            if let Some(doc_id) = body["query"]["term"]["document_id"].as_str() {
                // Honour `_source.excludes` the way OpenSearch does, so a
                // meta-only request is genuinely tested and not just assumed.
                let excludes: Vec<String> = body["_source"]["excludes"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let after = body["search_after"][0].as_i64();
                if after.is_some() {
                    return ResponseTemplate::new(200).set_body_json(json!({
                        "took": 1,
                        "hits": { "total": { "value": 2, "relation": "eq" }, "hits": [] }
                    }));
                }
                let hits: Vec<Value> = (0..2)
                    .map(|i| {
                        let mut source = serde_json::Map::new();
                        source.insert("chunk_index".into(), json!(i));
                        source.insert("document_id".into(), json!(doc_id));
                        source.insert("content".into(), json!(format!("chunk {i} body text")));
                        source.insert("blurb".into(), json!("blurb"));
                        source.insert("title".into(), json!("Title"));
                        source.insert("semantic_identifier".into(), json!("Title"));
                        source.insert("source_type".into(), json!("web"));
                        source.insert("hidden".into(), json!(false));
                        source.insert("content_vector".into(), json!([0.1, 0.2, 0.3]));
                        for field in &excludes {
                            source.remove(field.as_str());
                        }
                        json!({ "_id": format!("{doc_id}__{i}"), "_source": source })
                    })
                    .collect();
                return ResponseTemplate::new(200).set_body_json(json!({
                    "took": 1,
                    "hits": { "total": { "value": 2, "relation": "eq" }, "hits": hits }
                }));
            }

            // Content search.
            ResponseTemplate::new(200).set_body_json(json!({
                "took": 5,
                "hits": {
                    "total": { "value": 1, "relation": "eq" },
                    "hits": [{
                        "_score": 9.5,
                        "_source": {
                            "document_id": "https://example.com/ccc",
                            "chunk_index": 1,
                            "semantic_identifier": "Newest Page",
                            "source_type": "web",
                            "blurb": "blurb"
                        },
                        "highlight": { "content": ["a <em>match</em>"] }
                    }]
                }
            }))
        })
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r".*/_mapping$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            INDEX: { "mappings": { "properties": {
                "embeddings": { "properties": { "full_embedding": { "type": "knn_vector", "dimension": 768 } } },
                "content_vector": { "type": "float" },
                "content": { "type": "text" },
                "document_id": { "type": "keyword" }
            }}}
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r".*/_doc/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "found": true,
            "_source": { "content_vector": vec![0.5f32; 768] }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r".*/_settings$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            INDEX: { "settings": { "index": { "number_of_shards": "1" } } }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/_cat/indices/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "index": INDEX, "docs.count": "20", "docs.deleted": "1", "store.size": "4096"
        }])))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/_cat/allocation.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "disk.total": "1000", "disk.avail": "400", "disk.percent": "60"
        }])))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/_cluster/health.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "green" })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path_regex(r".*_delete_by_query.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": 2 })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path_regex(r".*_update_by_query.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "updated": 2 })))
        .mount(&server)
        .await;

    // Root ping, for /system/health.
    Mock::given(method("GET"))
        .and(path_regex(r"^/$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "version": { "number": "3.0.0" } })))
        .mount(&server)
        .await;

    server
}

async fn harness_with(mut configure: impl FnMut(&mut ServerConfig)) -> Option<Harness> {
    let guard = EXCLUSIVE.lock().await;
    let dsn = std::env::var("OVIS_TEST_DATABASE_URL").ok()?;
    if dsn.trim().is_empty() {
        return None;
    }

    let os_server = mock_opensearch().await;

    let mut cfg = ServerConfig {
        database_url: dsn,
        opensearch_url: os_server.uri(),
        ..Default::default()
    };
    configure(&mut cfg);
    let cfg = Arc::new(cfg);

    let db = ovis_core::db::create_pg_pool(&cfg.database_url, 5)
        .await
        .expect("test database");
    reseed(&db).await;

    let os = OsClient::new(&cfg.opensearch_url, None, None).unwrap();
    let runtime = RuntimeMeta {
        index_name: INDEX.to_string(),
        embedding_model: "snowflake-arctic-embed:m".into(),
        embedding_dim: 768,
        query_prefix: String::new(),
        search_settings_id: 2,
        capabilities: IndexCapabilities {
            knn_field: None,
            source_vector_field: Some("content_vector".into()),
        },
        schema: SchemaProbe::default(),
        refreshed_at: chrono::Utc::now(),
    };

    let state = AppState {
        db,
        os,
        onyx: None,
        embed: None,
        caches: Caches::new(),
        runtime: Arc::new(ArcSwap::from_pointee(runtime)),
        cfg,
        build: BuildInfo::current(),
        pending_deletes_enabled: false,
        metrics: None,
    };

    Some(Harness {
        app: ovis_backend::app(state.clone()),
        state,
        _os: os_server,
        _guard: guard,
    })
}

async fn harness() -> Option<Harness> {
    harness_with(|_| {}).await
}

async fn reseed(pool: &sqlx::PgPool) {
    for table in [
        "document__tag",
        "chunk_stats",
        "document_retrieval_feedback",
        "document_by_connector_credential_pair",
        "index_attempt_errors",
        "index_attempt",
        "background_error",
        "document",
        "tag",
        "connector_credential_pair",
        "connector",
        "credential",
        "search_settings",
    ] {
        sqlx::query(&format!("DELETE FROM public.{table}"))
            .execute(pool)
            .await
            .unwrap_or_else(|e| panic!("clearing {table}: {e}"));
    }
    sqlx::raw_sql(include_str!("../../../tests/fixtures/seed.sql"))
        .execute(pool)
        .await
        .expect("seeding");
}

fn skip(test: &str) {
    eprintln!("SKIPPED {test}: set OVIS_TEST_DATABASE_URL (see `scripts/test-db.sh up`)");
}

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("expected JSON, got {:?}: {}", self.body, e))
    }
    fn error_code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or("").to_string()
    }
    fn req_id(&self) -> String {
        self.json()["error"]["req_id"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
}

async fn send(app: &axum::Router, request: Request<Body>) -> Reply {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).to_string(),
    }
}

async fn get(app: &axum::Router, uri: &str) -> Reply {
    send(
        app,
        Request::builder().uri(uri).body(Body::empty()).unwrap(),
    )
    .await
}

async fn send_json(app: &axum::Router, method: &str, uri: &str, body: Value) -> Reply {
    send(
        app,
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

/// A document id, percent-encoded as the API requires.
fn enc(id: &str) -> String {
    percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC).to_string()
}

const NEWEST: &str = "https://example.com/ccc";
const OLDEST: &str = "https://example.com/aaa";
const TRICKY: &str = "https://example.com/tricky?a=1&b=2 c=café";
const DELETE_ME: &str = "https://example.com/deleteme";

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_returns_the_documented_envelope_newest_first() {
    let Some(h) = harness().await else {
        return skip("list_returns_the_documented_envelope_newest_first");
    };

    let reply = get(&h.app, "/api/v1/pages?limit=5").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();

    for field in [
        "items",
        "total",
        "total_exact",
        "page",
        "limit",
        "next_cursor",
        "has_more",
    ] {
        assert!(body.get(field).is_some(), "missing envelope field {field}");
    }
    assert_eq!(body["limit"], 5);
    assert_eq!(body["page"], 1);
    assert_eq!(body["items"][0]["id"], NEWEST, "not ordered by recency");

    let item = &body["items"][0];
    for field in [
        "id",
        "semantic_id",
        "link",
        "updated_at",
        "last_modified",
        "chunk_count",
        "boost",
        "hidden",
        "connector_id",
        "connector_name",
        "connector_source",
        "metadata",
    ] {
        assert!(item.get(field).is_some(), "item is missing {field}");
    }
    // The list path reports chunk counts from Postgres and never calls the index.
    assert_eq!(item["chunk_count"], 7);
}

#[tokio::test]
async fn cursor_paging_walks_the_whole_set_without_gaps() {
    let Some(h) = harness().await else {
        return skip("cursor_paging_walks_the_whole_set_without_gaps");
    };

    let mut seen: Vec<String> = Vec::new();
    let mut uri = "/api/v1/pages?limit=3".to_string();
    for _ in 0..10 {
        let body = get(&h.app, &uri).await.json();
        for item in body["items"].as_array().unwrap() {
            seen.push(item["id"].as_str().unwrap().to_string());
        }
        match body["next_cursor"].as_str() {
            Some(cursor) => uri = format!("/api/v1/pages?limit=3&cursor={}", enc(cursor)),
            None => break,
        }
    }
    assert_eq!(seen.len(), 10, "visited {} of 10 documents", seen.len());
    let unique: std::collections::HashSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), 10, "a document was returned twice");
}

#[tokio::test]
async fn a_cursor_from_one_sort_is_rejected_by_another() {
    let Some(h) = harness().await else {
        return skip("a_cursor_from_one_sort_is_rejected_by_another");
    };

    let cursor = get(&h.app, "/api/v1/pages?limit=2")
        .await
        .json()["next_cursor"]
        .as_str()
        .unwrap()
        .to_string();

    let reply = get(
        &h.app,
        &format!("/api/v1/pages?limit=2&sort=chunks_desc&cursor={}", enc(&cursor)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.error_code(), "BAD_REQUEST");
    assert!(reply.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("updated_desc"));
}

#[tokio::test]
async fn server_side_presets_replace_the_uis_client_side_filtering() {
    let Some(h) = harness().await else {
        return skip("server_side_presets_replace_the_uis_client_side_filtering");
    };

    let stubs = get(&h.app, "/api/v1/pages?chunk_min=0&chunk_max=0").await.json();
    assert_eq!(stubs["total"], 1);
    assert_eq!(stubs["items"][0]["chunk_count"], 0);

    let heavy = get(&h.app, "/api/v1/pages?chunk_min=11").await.json();
    assert_eq!(heavy["total"], 1);
    assert_eq!(heavy["items"][0]["chunk_count"], 12);

    let hidden = get(&h.app, "/api/v1/pages?hidden=true").await.json();
    assert_eq!(hidden["total"], 1);
    assert_eq!(hidden["items"][0]["hidden"], true);

    let github = get(&h.app, "/api/v1/pages?source=github").await.json();
    assert_eq!(github["total"], 1);
    assert_eq!(github["items"][0]["connector_source"], "GITHUB");
}

#[tokio::test]
async fn unknown_query_parameters_are_rejected_rather_than_ignored() {
    let Some(h) = harness().await else {
        return skip("unknown_query_parameters_are_rejected_rather_than_ignored");
    };

    // A typo must not silently produce a differently-ordered page.
    for uri in [
        "/api/v1/pages?sortt=updated_desc",
        "/api/v1/pages?limitt=5",
        "/api/v1/search?q=x&moode=hybrid",
        "/api/v1/tags?keyy=author",
        "/api/v1/stats/timeline?windoww=24h",
    ] {
        let reply = get(&h.app, uri).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(reply.error_code(), "BAD_REQUEST", "{uri}");
    }
}

// ---------------------------------------------------------------------------
// Detail, chunks, vector, text
// ---------------------------------------------------------------------------

#[tokio::test]
async fn detail_carries_the_full_documented_shape() {
    let Some(h) = harness().await else {
        return skip("detail_carries_the_full_documented_shape");
    };

    let body = get(&h.app, &format!("/api/v1/pages/{}", enc(NEWEST)))
        .await
        .json();
    for field in [
        "id",
        "semantic_id",
        "primary_owners",
        "secondary_owners",
        "content_hash",
        "from_ingestion_api",
        "cc_pair_id",
        "cc_pair_status",
        "tags",
        "pg_row",
        "recrawl_risk",
    ] {
        assert!(body.get(field).is_some(), "detail is missing {field}");
    }
    assert_eq!(body["pg_row"], true);
    assert_eq!(body["cc_pair_status"], "ACTIVE");
    assert_eq!(body["recrawl_risk"], true);
    // Detail is a metadata call: no chunk content.
    assert!(body.get("chunks").is_none());
    assert!(body.get("full_text").is_none());
}

#[tokio::test]
async fn a_document_id_with_a_query_string_and_unicode_survives_the_round_trip() {
    let Some(h) = harness().await else {
        return skip("a_document_id_with_a_query_string_and_unicode_survives_the_round_trip");
    };

    let reply = get(&h.app, &format!("/api/v1/pages/{}", enc(TRICKY))).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["id"], TRICKY);
}

#[tokio::test]
async fn an_unencoded_document_id_is_a_400_that_says_how_to_fix_it() {
    let Some(h) = harness().await else {
        return skip("an_unencoded_document_id_is_a_400_that_says_how_to_fix_it");
    };

    // Without the API-scoped fallback this would reach the SPA handler and answer
    // 200 with an HTML document, which surfaces as a JSON parse error far from
    // the actual mistake.
    let reply = get(&h.app, "/api/v1/pages/https://example.com/ccc").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("percent-encoded"));
    assert!(!reply.body.contains("<html"));
}

#[tokio::test]
async fn an_unknown_api_route_returns_json_not_the_spa_shell() {
    let Some(h) = harness().await else {
        return skip("an_unknown_api_route_returns_json_not_the_spa_shell");
    };

    let reply = get(&h.app, "/api/v1/nope").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.error_code(), "NOT_FOUND");
}

#[tokio::test]
async fn chunks_are_paged_and_never_carry_vectors() {
    let Some(h) = harness().await else {
        return skip("chunks_are_paged_and_never_carry_vectors");
    };

    let body = get(
        &h.app,
        &format!("/api/v1/pages/{}/chunks?limit=2", enc(NEWEST)),
    )
    .await
    .json();

    assert_eq!(body["total_chunks"], 2);
    assert_eq!(body["embedding_model"], "snowflake-arctic-embed:m");
    assert_eq!(body["embedding_dim"], 768);
    let chunk = &body["items"][0];
    assert_eq!(chunk["chunk_index"], 0);
    assert_eq!(chunk["token_estimate"], 4);
    // The old detail response shipped full embedding arrays plus a redundant
    // 6-float sample alongside them.
    let rendered = body.to_string();
    assert!(!rendered.contains("content_vector"), "a vector leaked into the chunk list");
    assert!(!rendered.contains("embedding_sample"));

    let meta_only = get(
        &h.app,
        &format!("/api/v1/pages/{}/chunks?include=meta_only", enc(NEWEST)),
    )
    .await
    .json();
    assert!(meta_only["items"][0]["content"].is_null());
}

#[tokio::test]
async fn a_single_chunk_vector_is_real_and_correctly_sized() {
    let Some(h) = harness().await else {
        return skip("a_single_chunk_vector_is_real_and_correctly_sized");
    };

    let body = get(
        &h.app,
        &format!("/api/v1/pages/{}/chunks/0/vector", enc(NEWEST)),
    )
    .await
    .json();
    assert_eq!(body["dim"], 768);
    assert_eq!(body["model"], "snowflake-arctic-embed:m");
    assert_eq!(body["vector"].as_array().unwrap().len(), 768);
}

#[tokio::test]
async fn text_is_plain_and_optionally_a_download() {
    let Some(h) = harness().await else {
        return skip("text_is_plain_and_optionally_a_download");
    };

    let reply = get(&h.app, &format!("/api/v1/pages/{}/text", enc(NEWEST))).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.headers.get("content-type").unwrap(),
        "text/plain; charset=utf-8"
    );
    assert!(reply.body.contains("chunk 0 body text"));
    assert!(reply.headers.get("content-disposition").is_none());

    let download = get(
        &h.app,
        &format!("/api/v1/pages/{}/text?download=1", enc(NEWEST)),
    )
    .await;
    let disposition = download
        .headers
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(disposition.starts_with("attachment;"));
    assert!(!disposition.contains('/'), "the filename must be path-safe");
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn patch_merges_metadata_and_reports_how_it_applied_the_change() {
    let Some(h) = harness().await else {
        return skip("patch_merges_metadata_and_reports_how_it_applied_the_change");
    };

    let reply = send_json(
        &h.app,
        "PATCH",
        &format!("/api/v1/pages/{}", enc(OLDEST)),
        json!({ "semantic_id": "Renamed", "hidden": true, "metadata_merge": { "note": "x" } }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["semantic_id"], "Renamed");
    assert_eq!(body["hidden"], true);
    assert_eq!(body["index_synced"], true);
    // No Onyx key configured, so boost/hidden went through direct SQL — and the
    // response says which path ran rather than leaving the caller guessing.
    assert_eq!(body["boost_hidden_via"], "direct_sql");
    // The pre-existing metadata key survived.
    assert_eq!(body["metadata"]["keep"], "me");
    assert_eq!(body["metadata"]["note"], "x");
}

#[tokio::test]
async fn patch_on_a_missing_document_is_404_and_an_empty_patch_is_400() {
    let Some(h) = harness().await else {
        return skip("patch_on_a_missing_document_is_404_and_an_empty_patch_is_400");
    };

    let missing = send_json(
        &h.app,
        "PATCH",
        &format!("/api/v1/pages/{}", enc("https://example.com/never")),
        json!({ "semantic_id": "x" }),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    let empty = send_json(
        &h.app,
        "PATCH",
        &format!("/api/v1/pages/{}", enc(OLDEST)),
        json!({}),
    )
    .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);

    let typo = send_json(
        &h.app,
        "PATCH",
        &format!("/api/v1/pages/{}", enc(OLDEST)),
        json!({ "hiden": true }),
    )
    .await;
    assert_eq!(typo.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_a_tagged_document_reports_exactly_what_happened() {
    let Some(h) = harness().await else {
        return skip("deleting_a_tagged_document_reports_exactly_what_happened");
    };

    let reply = send(
        &h.app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/pages/{}", enc(DELETE_ME)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["pg_deleted"], true);
    assert_eq!(body["chunks_deleted"], 2);
    assert_eq!(body["index_cleanup_pending"], false);
    // Its connector is ACTIVE, so the next refresh will likely bring it back.
    assert_eq!(body["recrawl_risk"], true);

    // Gone from the listing.
    let after = get(&h.app, "/api/v1/pages?limit=50").await.json();
    let ids: Vec<&str> = after["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&DELETE_ME));
}

#[tokio::test]
async fn a_delete_invalidates_the_cached_total() {
    let Some(h) = harness().await else {
        return skip("a_delete_invalidates_the_cached_total");
    };

    // A *filtered* total, because those are counted exactly. The unfiltered grand
    // total is deliberately a planner estimate flagged `total_exact: false`.
    let before = get(&h.app, "/api/v1/pages?hidden=false").await.json();
    assert_eq!(before["total_exact"], true);
    let count_before = before["total"].as_i64().unwrap();

    send(
        &h.app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/pages/{}", enc(DELETE_ME)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    let after = get(&h.app, "/api/v1/pages?hidden=false").await.json();
    assert_eq!(
        after["total"].as_i64().unwrap(),
        count_before - 1,
        "the cached total survived a delete, so the UI would show a stale number \
         immediately after its own action"
    );
}

#[tokio::test]
async fn the_unfiltered_grand_total_is_labelled_when_it_is_an_estimate() {
    let Some(h) = harness().await else {
        return skip("the_unfiltered_grand_total_is_labelled_when_it_is_an_estimate");
    };

    // An exact count(*) over 1.65M rows takes ~130 ms and would dominate an
    // otherwise sub-millisecond list response, so the grand total starts as the
    // planner estimate and says so.
    let first = get(&h.app, "/api/v1/pages?limit=1").await.json();
    assert!(first["total"].as_i64().unwrap() >= 0);
    assert!(first.get("total_exact").is_some());

    // The background exact count then takes over.
    for _ in 0..40 {
        let reply = get(&h.app, "/api/v1/pages?limit=1").await.json();
        if reply["total_exact"] == json!(true) {
            assert_eq!(reply["total"], 10, "the exact count disagrees with the fixture");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the background exact count never landed");
}

#[tokio::test]
async fn deleting_a_missing_document_is_a_404_not_a_fake_success() {
    let Some(h) = harness().await else {
        return skip("deleting_a_missing_document_is_a_404_not_a_fake_success");
    };

    let reply = send(
        &h.app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/pages/{}", enc("https://example.com/never")))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // The old handler collapsed every delete failure into a 404 *and* had a
    // fallback that reported success when the index happened to hold chunks.
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.error_code(), "NOT_FOUND");
}

#[tokio::test]
async fn batch_delete_reports_per_item_outcomes_and_partial_failure() {
    let Some(h) = harness().await else {
        return skip("batch_delete_reports_per_item_outcomes_and_partial_failure");
    };

    let reply = send_json(
        &h.app,
        "POST",
        "/api/v1/pages/batch-delete",
        json!({ "document_ids": [DELETE_ME, "https://example.com/never"] }),
    )
    .await;

    // Partial failure is not success: 207 says "read the per-item outcomes".
    assert_eq!(reply.status, StatusCode::MULTI_STATUS);
    let body = reply.json();
    assert_eq!(body["success"], false);
    assert_eq!(body["deleted"], 1);
    assert_eq!(body["failed"].as_array().unwrap().len(), 1);
    assert_eq!(body["failed"][0]["id"], "https://example.com/never");
    assert_eq!(body["failed"][0]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn a_fully_successful_batch_delete_is_a_200_with_success_true() {
    let Some(h) = harness().await else {
        return skip("a_fully_successful_batch_delete_is_a_200_with_success_true");
    };

    let reply = send_json(
        &h.app,
        "POST",
        "/api/v1/pages/batch-delete",
        json!({ "document_ids": [DELETE_ME] }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["success"], true);
    assert_eq!(body["deleted"], 1);
    assert!(body["failed"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_hydrates_hits_from_postgres_and_admits_degradation() {
    let Some(h) = harness().await else {
        return skip("search_hydrates_hits_from_postgres_and_admits_degradation");
    };

    let keyword = get(&h.app, "/api/v1/search?q=match&limit=5").await.json();
    assert_eq!(keyword["mode"], "keyword");
    assert!(keyword["degraded"].is_null());
    let hit = &keyword["items"][0];
    assert_eq!(hit["document_id"], NEWEST);
    assert_eq!(hit["snippet"], "a <em>match</em>");
    // Hydrated from Postgres, not guessed from the chunk.
    assert_eq!(hit["chunk_count"], 7);
    assert_eq!(hit["connector_name"], "tildes-like");
    assert_eq!(hit["semantic_id"], "Newest Page");

    // This index has no populated kNN field, so a semantic request must say it
    // fell back rather than returning nothing.
    let semantic = get(&h.app, "/api/v1/search?q=match&mode=semantic").await.json();
    assert_eq!(semantic["mode"], "semantic");
    assert_eq!(semantic["degraded"], "no_knn_field");
    assert!(
        !semantic["items"].as_array().unwrap().is_empty(),
        "a degraded search must still return keyword results"
    );
}

// ---------------------------------------------------------------------------
// Connectors, indexing, stats, tags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connector_summaries_expose_real_status_and_park_state() {
    let Some(h) = harness().await else {
        return skip("connector_summaries_expose_real_status_and_park_state");
    };

    let body = get(&h.app, "/api/v1/connectors").await.json();
    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 4);

    let find = |name: &str| items.iter().find(|c| c["name"] == name).unwrap();
    assert_eq!(find("paused-web")["status"], "PAUSED");
    assert_eq!(find("parked-web")["parked"], true);
    assert_eq!(find("tildes-like")["doc_count"], 8);
    assert_eq!(find("tildes-like")["refresh_freq_secs"], 2_592_000);
    assert!(find("tildes-like")["last_attempt"]["status"].is_string());
}

#[tokio::test]
async fn connector_sub_resources_are_paginated_and_label_their_window() {
    let Some(h) = harness().await else {
        return skip("connector_sub_resources_are_paginated_and_label_their_window");
    };

    let attempts = get(&h.app, "/api/v1/connectors/1/attempts?limit=2").await.json();
    assert_eq!(attempts["total"], 3);
    assert_eq!(attempts["items"].as_array().unwrap().len(), 2);
    assert_eq!(attempts["has_more"], true);

    let errors = get(&h.app, "/api/v1/connectors/2/errors").await.json();
    assert_eq!(
        errors["window"], "24h",
        "the rolling retention must be stated so an empty list is not misread"
    );

    let docs = get(&h.app, "/api/v1/connectors/1/docs?limit=3").await.json();
    assert_eq!(docs["total"], 8);
    assert_eq!(docs["items"].as_array().unwrap().len(), 3);

    let detail = get(&h.app, "/api/v1/connectors/1?history=7d").await.json();
    assert!(detail["history"].is_array());
    assert!(detail["connector_specific_config"]["base_url"].is_string());

    assert_eq!(
        get(&h.app, "/api/v1/connectors/9999").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn every_connector_action_refuses_cleanly_without_an_onyx_key() {
    let Some(h) = harness().await else {
        return skip("every_connector_action_refuses_cleanly_without_an_onyx_key");
    };

    for (method, uri, body) in [
        ("POST", "/api/v1/connectors/1/pause", json!(null)),
        ("POST", "/api/v1/connectors/1/resume", json!(null)),
        ("POST", "/api/v1/connectors/1/run-once", json!({})),
        ("POST", "/api/v1/connectors/1/prune", json!(null)),
        ("PATCH", "/api/v1/connectors/1", json!({ "name": "x" })),
        (
            "DELETE",
            "/api/v1/connectors/1",
            json!({ "confirm_name": "tildes-like" }),
        ),
        (
            "POST",
            "/api/v1/indexing/targeted-reindex",
            json!({ "cc_pair_id": 1, "only_failed": true }),
        ),
        ("GET", "/api/v1/indexing/failed-documents", json!(null)),
    ] {
        let reply = if body.is_null() {
            send(
                &h.app,
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
        } else {
            send_json(&h.app, method, uri, body).await
        };
        assert_eq!(
            reply.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {uri} should be 503 without a key, got {}",
            reply.status
        );
        assert_eq!(reply.error_code(), "ONYX_UNCONFIGURED", "{method} {uri}");
    }
}

#[tokio::test]
async fn indexing_telemetry_is_exposed_globally() {
    let Some(h) = harness().await else {
        return skip("indexing_telemetry_is_exposed_globally");
    };

    let all = get(&h.app, "/api/v1/indexing/attempts?limit=10").await.json();
    assert_eq!(all["total"], 6);

    let running = get(&h.app, "/api/v1/indexing/attempts?status=in_progress")
        .await
        .json();
    assert_eq!(running["total"], 2, "status filtering must be case-insensitive");
    let stalled: Vec<bool> = running["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["stalled"].as_bool().unwrap())
        .collect();
    assert!(stalled.contains(&true) && stalled.contains(&false));

    let one = get(&h.app, "/api/v1/indexing/attempts/1").await.json();
    assert_eq!(one["id"], 1);
    assert_eq!(
        get(&h.app, "/api/v1/indexing/attempts/9999").await.status,
        StatusCode::NOT_FOUND
    );

    let background = get(&h.app, "/api/v1/indexing/background-errors").await.json();
    assert_eq!(background.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn stats_endpoints_answer_with_live_numbers() {
    let Some(h) = harness().await else {
        return skip("stats_endpoints_answer_with_live_numbers");
    };

    let overview = get(&h.app, "/api/v1/stats/overview").await.json();
    assert_eq!(overview["connectors"]["paused"], 2);
    assert_eq!(overview["connectors"]["parked"], 1);
    assert_eq!(overview["embedding"]["dim"], 768);
    assert_eq!(overview["index"]["name"], INDEX);
    assert_eq!(overview["index"]["read_only"], false);
    assert_eq!(overview["crawl"]["attempts_in_progress"], 2);
    assert_eq!(overview["crawl"]["attempts_stalled"], 1);
    assert_eq!(overview["attempts"]["success"], 1);

    let index = get(&h.app, "/api/v1/stats/index").await.json();
    assert_eq!(index["docs"], 20);
    assert_eq!(index["disk_used_pct"], 60.0);

    let sources = get(&h.app, "/api/v1/stats/sources").await.json();
    let web = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["source"] == "WEB")
        .unwrap();
    assert_eq!(web["documents"], 9, "a multi-connector document was double-counted");

    let top = get(&h.app, "/api/v1/stats/connectors/top?limit=2").await.json();
    assert_eq!(top.as_array().unwrap().len(), 2);

    let timeline = get(&h.app, "/api/v1/stats/timeline?window=7d&bucket=1d")
        .await
        .json();
    assert_eq!(timeline["window"], "7d");
    assert_eq!(timeline["bucket"], "1d");
    assert!(timeline["items"].as_array().unwrap().len() >= 7);
}

#[tokio::test]
async fn tag_facets_are_served_and_cached() {
    let Some(h) = harness().await else {
        return skip("tag_facets_are_served_and_cached");
    };

    let facets = get(&h.app, "/api/v1/tags?limit=10").await.json();
    let items = facets.as_array().unwrap();
    assert!(items.iter().any(|f| f["key"] == "author" && f["doc_count"] == 2));

    let scoped = get(&h.app, "/api/v1/tags?key=author").await.json();
    assert!(scoped.as_array().unwrap().iter().all(|f| f["key"] == "author"));

    let keys = get(&h.app, "/api/v1/tags/keys").await.json();
    assert!(keys
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["key"] == "author" && k["distinct_values"].is_number()));
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_is_200_when_everything_answers() {
    let Some(h) = harness().await else {
        return skip("health_is_200_when_everything_answers");
    };

    let reply = get(&h.app, "/api/v1/system/health").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["postgres"]["status"], "ok");
    assert_eq!(body["opensearch"]["status"], "ok");
    assert_eq!(body["schema_ok"], true);
    // Unconfigured optional dependencies cost features, not health.
    assert_eq!(body["onyx_api"]["status"], "unconfigured");
    assert_eq!(body["embedder"]["status"], "unconfigured");
    assert_eq!(reply.headers.get("cache-control").unwrap(), "no-store");
}

#[tokio::test]
async fn health_is_503_when_the_schema_does_not_match() {
    let Some(h) = harness().await else {
        return skip("health_is_503_when_the_schema_does_not_match");
    };

    // The C8 regression: the old handler returned 200 with `status: "degraded"`,
    // so a Docker HEALTHCHECK passed with a dead dependency.
    let current = h.state.runtime();
    h.state.runtime.store(Arc::new(RuntimeMeta {
        schema: SchemaProbe {
            missing_columns: vec!["document.chunk_count".into()],
            ..Default::default()
        },
        ..(*current).clone()
    }));

    let reply = get(&h.app, "/api/v1/system/health").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    let body = reply.json();
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["schema_ok"], false);
    assert_eq!(body["missing_columns"][0], "document.chunk_count");
}

#[tokio::test]
async fn an_endpoint_whose_column_is_missing_returns_501_not_a_wrong_answer() {
    let Some(h) = harness().await else {
        return skip("an_endpoint_whose_column_is_missing_returns_501_not_a_wrong_answer");
    };

    let current = h.state.runtime();
    h.state.runtime.store(Arc::new(RuntimeMeta {
        schema: SchemaProbe {
            missing_columns: vec!["document.chunk_count".into()],
            ..Default::default()
        },
        ..(*current).clone()
    }));

    let reply = get(&h.app, "/api/v1/pages?limit=5").await;
    assert_eq!(reply.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(reply.error_code(), "SCHEMA_MISMATCH");
}

#[tokio::test]
async fn delete_refuses_when_a_new_foreign_key_child_appears() {
    let Some(h) = harness().await else {
        return skip("delete_refuses_when_a_new_foreign_key_child_appears");
    };

    let current = h.state.runtime();
    h.state.runtime.store(Arc::new(RuntimeMeta {
        schema: SchemaProbe {
            unhandled_fk_children: vec!["document_annotation.document_id".into()],
            ..Default::default()
        },
        ..(*current).clone()
    }));

    let reply = send(
        &h.app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/pages/{}", enc(DELETE_ME)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // Better a loud refusal than a transaction that explodes halfway through.
    assert_eq!(reply.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(reply.error_code(), "SCHEMA_MISMATCH");
    assert!(reply.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("document_annotation"));
}

#[tokio::test]
async fn version_and_runtime_report_what_is_actually_running() {
    let Some(h) = harness().await else {
        return skip("version_and_runtime_report_what_is_actually_running");
    };

    let version = get(&h.app, "/api/v1/system/version").await.json();
    for field in ["version", "git_sha", "rustc", "built_at", "profile"] {
        assert!(
            !version[field].as_str().unwrap_or("").is_empty(),
            "{field} is empty"
        );
    }

    let runtime = get(&h.app, "/api/v1/system/runtime").await.json();
    assert_eq!(runtime["index_name"], INDEX);
    assert_eq!(runtime["embedding_dim"], 768);
    assert_eq!(runtime["search_settings_id"], 2);
    // The UI footer reads this instead of hardcoding `danswer_chunk`.
    assert!(!runtime["index_name"].as_str().unwrap().contains('*'));
}

#[tokio::test]
async fn every_error_response_carries_a_correlatable_request_id() {
    let Some(h) = harness().await else {
        return skip("every_error_response_carries_a_correlatable_request_id");
    };

    for uri in [
        "/api/v1/pages?sortt=x",
        "/api/v1/nope",
        "/api/v1/search",
        "/api/v1/connectors/9999",
    ] {
        let reply = get(&h.app, uri).await;
        assert!(reply.status.is_client_error(), "{uri}");
        let req_id = reply.req_id();
        assert!(!req_id.is_empty() && req_id != "-", "{uri}: req_id={req_id:?}");
        // And the same id is on the response header, so a client can quote it.
        assert_eq!(
            reply.headers.get("x-request-id").unwrap().to_str().unwrap(),
            req_id,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn an_inbound_request_id_is_honoured_and_sanitised() {
    let Some(h) = harness().await else {
        return skip("an_inbound_request_id_is_honoured_and_sanitised");
    };

    let reply = send(
        &h.app,
        Request::builder()
            .uri("/api/v1/nope")
            .header("x-request-id", "proxy-abc-123")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(reply.req_id(), "proxy-abc-123");

    // A header-injection attempt must not survive into the log or the response.
    let nasty = send(
        &h.app,
        Request::builder()
            .uri("/api/v1/nope")
            .header("x-request-id", "abc\tdef")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(!nasty.req_id().contains('\t'));
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_guards_everything_except_the_health_probe() {
    let Some(h) = harness_with(|cfg| cfg.api_token = Some("s3cret".into())).await else {
        return skip("auth_guards_everything_except_the_health_probe");
    };

    // Health stays open so a container probe needs no credential.
    assert_eq!(
        get(&h.app, "/api/v1/system/health").await.status,
        StatusCode::OK
    );

    for uri in [
        "/api/v1/pages",
        "/api/v1/connectors",
        "/api/v1/system/runtime",
        "/api/v1/system/metrics",
        "/api/v1/stats/overview",
    ] {
        let reply = get(&h.app, uri).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(reply.error_code(), "UNAUTHORIZED", "{uri}");
    }

    let authorized = send(
        &h.app,
        Request::builder()
            .uri("/api/v1/pages?limit=1")
            .header("authorization", "Bearer s3cret")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(authorized.status, StatusCode::OK);

    let wrong = send(
        &h.app,
        Request::builder()
            .uri("/api/v1/pages?limit=1")
            .header("authorization", "Bearer s3crev")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    // EventSource cannot set headers, so SSE also accepts ?token=.
    let sse = get(&h.app, "/api/v1/pages/stream?limit=1&token=s3cret").await;
    assert_eq!(sse.status, StatusCode::OK);
    let sse_wrong = get(&h.app, "/api/v1/pages/stream?limit=1&token=nope").await;
    assert_eq!(sse_wrong.status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// SSE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sse_stream_emits_the_documented_contract() {
    let Some(h) = harness().await else {
        return skip("sse_stream_emits_the_documented_contract");
    };

    let reply = get(&h.app, "/api/v1/pages/stream?limit=4").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

    let wire = &reply.body;
    assert_eq!(wire.matches("event: page").count(), 4);
    assert!(wire.contains("id: 0"), "page events must be numbered");
    assert!(wire.contains("id: 3"));
    assert!(wire.contains("event: done"));
    assert!(!wire.contains("event: error"));

    // The terminal event reports what was actually sent.
    let done = wire
        .split("event: done")
        .nth(1)
        .unwrap()
        .lines()
        .find(|l| l.starts_with("data: "))
        .unwrap()
        .trim_start_matches("data: ");
    let payload: Value = serde_json::from_str(done).unwrap();
    assert_eq!(payload["total_matched"], 4);
    assert!(payload["time_ms"].is_number());

    // Events carry the same item shape as the list endpoint.
    let first = wire
        .lines()
        .find(|l| l.starts_with("data: "))
        .unwrap()
        .trim_start_matches("data: ");
    let item: Value = serde_json::from_str(first).unwrap();
    assert_eq!(item["id"], NEWEST);
    assert!(item["chunk_count"].is_number());
}

#[tokio::test]
async fn a_bad_stream_parameter_is_an_http_400_not_a_200_with_an_error_frame() {
    let Some(h) = harness().await else {
        return skip("a_bad_stream_parameter_is_an_http_400_not_a_200_with_an_error_frame");
    };

    let reply = get(&h.app, "/api/v1/pages/stream?sort=nope").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.error_code(), "BAD_REQUEST");
}

#[tokio::test]
async fn the_stream_never_emits_more_than_the_requested_limit() {
    let Some(h) = harness().await else {
        return skip("the_stream_never_emits_more_than_the_requested_limit");
    };

    // Fewer rows than the limit: the stream must end, not spin.
    let all = get(&h.app, "/api/v1/pages/stream?limit=1000").await;
    assert_eq!(all.body.matches("event: page").count(), 10);

    // More rows than the limit: exactly the limit.
    let capped = get(&h.app, "/api/v1/pages/stream?limit=3").await;
    assert_eq!(capped.body.matches("event: page").count(), 3);
}

// ---------------------------------------------------------------------------
// Honest failure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_dead_database_is_a_500_not_an_empty_200() {
    let Some(h) = harness().await else {
        return skip("a_dead_database_is_a_500_not_an_empty_200");
    };

    // The C1 regression, and the exact inverse of what the old test suite
    // asserted: it pointed at a nonexistent database and checked for
    // `200 OK {"total": 0, "items": []}`.
    h.state.db.close().await;

    for uri in [
        "/api/v1/pages?limit=5",
        "/api/v1/connectors",
        "/api/v1/tags",
        "/api/v1/indexing/attempts",
    ] {
        let reply = get(&h.app, uri).await;
        assert_eq!(
            reply.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{uri} returned {} instead of 500",
            reply.status
        );
        assert_eq!(reply.error_code(), "DATABASE", "{uri}");
        // The driver's message — which can carry the host and DSN — must not be
        // echoed to the caller.
        let message = reply.json()["error"]["message"].as_str().unwrap().to_string();
        assert_eq!(message, "database error");
        assert!(!message.contains("postgres://"));
    }

    // And health flips to 503, so a container probe restarts the process.
    let health = get(&h.app, "/api/v1/system/health").await;
    assert_eq!(health.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(health.json()["postgres"]["status"], "down");
}

#[tokio::test]
async fn a_dead_search_index_is_a_502_and_does_not_take_the_list_path_down() {
    let Some(h) = harness_with(|cfg| {
        // Nothing listening: the client fails fast rather than hanging.
        cfg.opensearch_url = "http://127.0.0.1:1".into();
    })
    .await
    else {
        return skip("a_dead_search_index_is_a_502_and_does_not_take_the_list_path_down");
    };

    // The list path makes no index calls at all, so it still works.
    let list = get(&h.app, "/api/v1/pages?limit=5").await;
    assert_eq!(
        list.status,
        StatusCode::OK,
        "listing must not depend on the search index"
    );

    // Content search does depend on it, and says so honestly.
    let search = get(&h.app, "/api/v1/search?q=anything").await;
    assert_eq!(search.status, StatusCode::BAD_GATEWAY);
    assert_eq!(search.error_code(), "OPENSEARCH_UPSTREAM");
    assert_eq!(
        search.json()["error"]["message"].as_str().unwrap(),
        "search index unavailable"
    );

    let health = get(&h.app, "/api/v1/system/health").await;
    assert_eq!(health.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(health.json()["opensearch"]["status"], "down");
}
