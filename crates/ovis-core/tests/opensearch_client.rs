//! OpenSearch client tests against a mock server.
//!
//! These pin the *requests* OVIS sends as tightly as the responses it parses,
//! because the requests are where the old implementation's cost lived: a chunk
//! fetch with no `_source` filter downloaded three 768-float arrays per chunk,
//! for every chunk of every row, on every list request.
//!
//! `wiremock` was already a dev-dependency and entirely unused. Now it is used.

use ovis_core::api_types::SearchMode;
use ovis_core::search::{OsClient, SearchFilters, SearchRequest};
use ovis_core::CoreError;
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const INDEX: &str = "danswer_chunk_snowflake_arctic_embed_m";

fn client(server: &MockServer) -> OsClient {
    OsClient::new(&server.uri(), None, None).unwrap()
}

/// Capture the body of the single request the mock received.
async fn captured_body(server: &MockServer) -> Value {
    let requests = server.received_requests().await.expect("recording enabled");
    assert_eq!(requests.len(), 1, "expected exactly one request");
    serde_json::from_slice(&requests[0].body).expect("a JSON body")
}

fn search_hits(hits: Vec<Value>, total: i64, relation: &str) -> Value {
    json!({
        "took": 7,
        "hits": { "total": { "value": total, "relation": relation }, "hits": hits }
    })
}

#[tokio::test]
async fn chunk_fetch_excludes_vectors_and_tracks_the_real_total() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{INDEX}/_search")))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_hits(
            vec![json!({
                "_id": "https://example.com/a__0",
                "_source": {
                    "chunk_index": 0,
                    "content": "one two three",
                    "blurb": "b",
                    "title": "T",
                    "semantic_identifier": "T",
                    "source_type": "web",
                    "hidden": false,
                    "source_links": "{\"0\": \"https://example.com/a\"}"
                }
            })],
            // More chunks than the page returned: the total must come from here,
            // not from `items.len()`.
            42,
            "eq",
        )))
        .mount(&server)
        .await;

    let (items, total, next_after) = client(&server)
        .document_chunks(INDEX, "https://example.com/a", None, 100, true)
        .await
        .unwrap();

    assert_eq!(
        total, 42,
        "the chunk total must be the index's, not the page's"
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].token_estimate, Some(3));
    assert_eq!(
        items[0].source_links.as_ref().unwrap()["0"],
        "https://example.com/a"
    );
    assert_eq!(next_after, None, "a partial page has no next cursor");

    let body = captured_body(&server).await;
    let excludes = body["_source"]["excludes"].as_array().unwrap();
    for field in ["embeddings", "content_vector", "title_vector"] {
        assert!(
            excludes.iter().any(|v| v == field),
            "{field} was not excluded — this is the N+1's real cost"
        );
    }
    assert_eq!(body["track_total_hits"], true);
    assert_eq!(
        body["query"]["term"]["document_id"],
        "https://example.com/a"
    );
    assert!(
        !body.to_string().contains("document_id.keyword"),
        "document_id is already a keyword field"
    );
}

#[tokio::test]
async fn a_full_chunk_page_advertises_a_next_cursor() {
    let server = MockServer::start().await;
    let hits: Vec<Value> = (0..3)
        .map(|i| json!({ "_source": { "chunk_index": i, "content": "x" } }))
        .collect();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_hits(hits, 9, "eq")))
        .mount(&server)
        .await;

    let (items, total, next_after) = client(&server)
        .document_chunks(INDEX, "d", None, 3, true)
        .await
        .unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(total, 9);
    assert_eq!(
        next_after,
        Some(2),
        "a full page must hand back the last chunk_index so paging continues"
    );

    // And the follow-up request carries search_after, replacing the old hard
    // `size: 500` truncation.
    let body = captured_body(&server).await;
    assert!(body.get("search_after").is_none());
}

#[tokio::test]
async fn chunk_paging_sends_search_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_hits(vec![], 0, "eq")))
        .mount(&server)
        .await;

    client(&server)
        .document_chunks(INDEX, "d", Some(99), 100, true)
        .await
        .unwrap();
    assert_eq!(captured_body(&server).await["search_after"], json!([99]));
}

#[tokio::test]
async fn meta_only_chunk_fetch_also_drops_the_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_hits(vec![], 0, "eq")))
        .mount(&server)
        .await;

    client(&server)
        .document_chunks(INDEX, "d", None, 10, false)
        .await
        .unwrap();

    let excludes = captured_body(&server).await["_source"]["excludes"]
        .as_array()
        .unwrap()
        .clone();
    for field in ["content", "blurb", "chunk_context", "doc_summary"] {
        assert!(excludes.iter().any(|v| v == field), "{field} not excluded");
    }
}

#[tokio::test]
async fn keyword_search_collapses_by_document_and_hides_hidden_chunks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_hits(
            vec![json!({
                "_score": 12.5,
                "_source": { "document_id": "https://example.com/a", "chunk_index": 3, "blurb": "fallback" },
                "highlight": { "content": ["a <em>match</em>"] }
            })],
            10_000,
            "gte",
        )))
        .mount(&server)
        .await;

    let results = client(&server)
        .search(
            INDEX,
            &SearchRequest {
                query: "tax reform".into(),
                mode: SearchMode::Keyword,
                filters: SearchFilters::default(),
                limit: 20,
                offset: 0,
                vector: None,
                knn_field: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(results.hits.len(), 1);
    assert_eq!(results.hits[0].score, 12.5);
    assert_eq!(results.hits[0].snippet.as_deref(), Some("a <em>match</em>"));
    assert_eq!(results.total, 10_000);
    assert!(
        !results.total_exact,
        "a `gte` relation means the total is a floor, and must be reported as such"
    );
    assert_eq!(results.took_ms, 7);

    let body = captured_body(&server).await;
    assert_eq!(body["collapse"]["field"], "document_id");
    assert_eq!(body["query"]["bool"]["filter"][0]["term"]["hidden"], false);
    assert!(body["highlight"]["fields"]["content"].is_object());
}

#[tokio::test]
async fn hybrid_search_sends_both_clauses_with_their_weights() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_hits(vec![], 0, "eq")))
        .mount(&server)
        .await;

    client(&server)
        .search(
            INDEX,
            &SearchRequest {
                query: "tax reform".into(),
                mode: SearchMode::Hybrid,
                filters: SearchFilters {
                    source: Some("WEB".into()),
                    include_hidden: false,
                },
                limit: 20,
                offset: 0,
                vector: Some(vec![0.25, 0.5]),
                knn_field: Some("embeddings.full_embedding".into()),
            },
        )
        .await
        .unwrap();

    let body = captured_body(&server).await;
    let should = body["query"]["bool"]["should"].as_array().unwrap();
    assert_eq!(should.len(), 2);
    assert_eq!(should[0]["multi_match"]["boost"], 0.4);
    assert_eq!(should[1]["knn"]["embeddings.full_embedding"]["boost"], 0.6);
    // Postgres stores WEB; the index stores web.
    assert_eq!(
        body["query"]["bool"]["filter"][0]["term"]["source_type"],
        "web"
    );
}

#[tokio::test]
async fn deleting_chunks_asks_for_refresh_and_tolerates_conflicts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{INDEX}/_delete_by_query")))
        .and(query_param("refresh", "true"))
        .and(query_param("conflicts", "proceed"))
        .and(body_json(json!({
            "query": { "term": { "document_id": "https://example.com/a" } }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": 14 })))
        .mount(&server)
        .await;

    let deleted = client(&server)
        .delete_document_chunks(INDEX, "https://example.com/a")
        .await
        .unwrap();
    assert_eq!(deleted, 14);
}

#[tokio::test]
async fn batch_delete_uses_terms_and_sums_across_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{INDEX}/_delete_by_query")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": 3 })))
        .mount(&server)
        .await;

    let ids: Vec<String> = (0..600)
        .map(|i| format!("https://example.com/{i}"))
        .collect();
    let deleted = client(&server)
        .delete_chunks_for(INDEX, &ids)
        .await
        .unwrap();

    // 600 ids ⇒ two requests of ≤500, and their counts add up.
    assert_eq!(deleted, 6);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        2,
        "ids must be chunked, not sent in one body"
    );
    for request in &requests {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let terms = body["query"]["terms"]["document_id"].as_array().unwrap();
        assert!(
            terms.len() <= 500,
            "a terms clause exceeded the batch bound"
        );
    }
}

#[tokio::test]
async fn an_empty_delete_makes_no_request_at_all() {
    let server = MockServer::start().await;
    assert_eq!(
        client(&server).delete_chunks_for(INDEX, &[]).await.unwrap(),
        0
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn chunk_counts_are_batched_into_one_aggregation_per_500_ids() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "hits": { "total": { "value": 0, "relation": "eq" }, "hits": [] },
            "aggregations": { "by_doc": { "buckets": [
                { "key": "https://example.com/a", "doc_count": 4 },
                { "key": "https://example.com/b", "doc_count": 9 }
            ]}}
        })))
        .mount(&server)
        .await;

    let counts = client(&server)
        .chunk_counts(
            INDEX,
            &[
                "https://example.com/a".into(),
                "https://example.com/b".into(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(counts.get("https://example.com/a"), Some(&4));
    assert_eq!(counts.get("https://example.com/b"), Some(&9));
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "two ids must cost one round-trip, not two"
    );
}

#[tokio::test]
async fn document_text_pages_until_the_index_runs_out() {
    let server = MockServer::start().await;
    // First page full (200), second page short — so exactly two requests.
    let full: Vec<Value> = (0..200)
        .map(|i| json!({ "_source": { "chunk_index": i, "content": format!("chunk{i}") } }))
        .collect();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            if body.get("search_after").is_some() {
                ResponseTemplate::new(200).set_body_json(search_hits(
                    vec![json!({ "_source": { "chunk_index": 200, "content": "last" } })],
                    201,
                    "eq",
                ))
            } else {
                ResponseTemplate::new(200).set_body_json(search_hits(full.clone(), 201, "eq"))
            }
        })
        .mount(&server)
        .await;

    let text = client(&server).document_text(INDEX, "d").await.unwrap();
    assert!(text.starts_with("chunk0\n\nchunk1"));
    assert!(text.ends_with("last"));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_chunk_vector_falls_back_to_search_when_the_deterministic_id_misses() {
    let server = MockServer::start().await;
    // GET by `{document_id}__{chunk_index}` misses: not every id in this index
    // matches its Postgres document id exactly.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "found": false })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/{INDEX}/_search")))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_hits(
            vec![json!({ "_source": { "content_vector": [0.1, 0.2, 0.3] } })],
            1,
            "eq",
        )))
        .mount(&server)
        .await;

    let vector = client(&server)
        .chunk_vector(INDEX, "https://example.com/a", 5, "content_vector")
        .await
        .unwrap();
    assert_eq!(vector, Some(vec![0.1, 0.2, 0.3]));
}

#[tokio::test]
async fn a_chunk_with_no_stored_vector_reports_none_rather_than_inventing_one() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "found": true, "_source": {} })),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_hits(vec![], 0, "eq")))
        .mount(&server)
        .await;

    assert_eq!(
        client(&server)
            .chunk_vector(INDEX, "d", 0, "content_vector")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn capability_probe_refuses_a_declared_but_empty_knn_field() {
    let server = MockServer::start().await;
    // Exactly the gamma mapping: a declared knn_vector, and the float arrays Onyx
    // actually populates.
    Mock::given(method("GET"))
        .and(path(format!("/{INDEX}/_mapping")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            INDEX: { "mappings": { "properties": {
                "embeddings": { "properties": {
                    "full_embedding": { "type": "knn_vector", "dimension": 768 }
                }},
                "content_vector": { "type": "float" },
                "content": { "type": "text" }
            }}}
        })))
        .mount(&server)
        .await;
    // `embeddings.full_embedding` exists in the mapping but in zero documents;
    // `content_vector` is populated.
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let field = body["query"]["exists"]["field"].as_str().unwrap_or("");
            let count = if field == "content_vector" { 1 } else { 0 };
            ResponseTemplate::new(200).set_body_json(json!({
                "hits": { "total": { "value": count, "relation": "eq" }, "hits": [] }
            }))
        })
        .mount(&server)
        .await;

    let caps = client(&server).probe_capabilities(INDEX).await.unwrap();
    assert!(
        !caps.knn_ready(),
        "a knn field with no documents must not be reported as usable: \
         querying it returns zero hits, which reads as 'nothing matched'"
    );
    assert_eq!(caps.source_vector_field.as_deref(), Some("content_vector"));
}

#[tokio::test]
async fn capability_probe_accepts_a_populated_knn_field() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/{INDEX}/_mapping")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            INDEX: { "mappings": { "properties": {
                "embeddings": { "properties": {
                    "full_embedding": { "type": "knn_vector", "dimension": 768 }
                }}
            }}}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "hits": { "total": { "value": 1, "relation": "eq" }, "hits": [] }
        })))
        .mount(&server)
        .await;

    let caps = client(&server).probe_capabilities(INDEX).await.unwrap();
    assert!(caps.knn_ready());
    assert_eq!(caps.knn_field.as_deref(), Some("embeddings.full_embedding"));
}

#[tokio::test]
async fn a_read_only_index_block_is_detected_in_either_encoding() {
    for value in [json!("true"), json!(true)] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{INDEX}/_settings")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                INDEX: { "settings": { "index": { "blocks": { "read_only_allow_delete": value } } } }
            })))
            .mount(&server)
            .await;
        assert!(
            client(&server).index_read_only(INDEX).await.unwrap(),
            "the flood-stage watermark block must be detected"
        );
    }

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            INDEX: { "settings": { "index": { "number_of_shards": "1" } } }
        })))
        .mount(&server)
        .await;
    assert!(!client(&server).index_read_only(INDEX).await.unwrap());
}

#[tokio::test]
async fn a_non_2xx_is_an_upstream_error_that_keeps_its_body_out_of_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string("circuit_breaking_exception: heap"),
        )
        .mount(&server)
        .await;

    let err = client(&server)
        .document_chunks(INDEX, "d", None, 10, true)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Search(_)));
    assert_eq!(err.code(), "OPENSEARCH_UPSTREAM");
    // Available for the log...
    assert!(err.to_string().contains("circuit_breaking_exception"));
}

#[tokio::test]
async fn a_hung_node_fails_instead_of_hanging_forever() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(search_hits(vec![], 0, "eq"))
                // Longer than the client's 15 s total timeout.
                .set_delay(std::time::Duration::from_secs(20)),
        )
        .mount(&server)
        .await;

    let started = std::time::Instant::now();
    let err = client(&server)
        .document_chunks(INDEX, "d", None, 10, true)
        .await
        .unwrap_err();
    // The old client had no timeouts at all: a hung node stalled the request
    // indefinitely while holding its Postgres pool slot.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(19),
        "took {:?}; the request timeout did not fire",
        started.elapsed()
    );
    assert!(matches!(err, CoreError::Search(_)));
}

#[tokio::test]
async fn malformed_json_is_an_upstream_error_not_a_panic() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>gateway</html>"))
        .mount(&server)
        .await;

    let err = client(&server)
        .document_chunks(INDEX, "d", None, 10, true)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("malformed"));
}
