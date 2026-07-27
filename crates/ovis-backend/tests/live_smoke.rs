//! Read-only smoke tests against the live deployment.
//!
//! `#[ignore]` by default, because they need the homelab. Run them deliberately:
//!
//! ```text
//! OVIS_SMOKE_URL=http://127.0.0.1:8791 cargo test -p ovis-backend --test live_smoke -- --ignored
//! ```
//!
//! Strictly read-only: no deletes, no edits, no connector actions. They exist to
//! answer "is the thing that is running actually correct against 1.65M real
//! documents", which no amount of fixture testing can.
//!
//! The old suite had a live-network test that was *not* `#[ignore]`d and silently
//! no-op'd when the host was unreachable, so it passed either way. These fail
//! loudly if the URL is set and the server is wrong, and skip only when the URL is
//! absent.

use serde_json::Value;

fn base_url() -> Option<String> {
    std::env::var("OVIS_SMOKE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

fn skip(test: &str) {
    eprintln!("SKIPPED {test}: set OVIS_SMOKE_URL to a running OVIS server");
}

async fn get(base: &str, path: &str) -> Value {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();
    let mut request = client.get(format!("{base}{path}"));
    if let Ok(token) = std::env::var("OVIS_API_TOKEN") {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response = request.send().await.expect("request");
    let status = response.status();
    let body = response.text().await.expect("body");
    assert!(
        status.is_success(),
        "GET {path} -> {status}: {}",
        &body[..body.len().min(400)]
    );
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("GET {path}: {e}: {body}"))
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn health_reports_every_dependency_and_a_real_index_name() {
    let Some(base) = base_url() else {
        return skip("health_reports_every_dependency_and_a_real_index_name");
    };

    let health = get(&base, "/api/v1/system/health").await;
    assert_eq!(health["status"], "ok", "the deployment is degraded: {health}");
    assert_eq!(health["postgres"]["status"], "ok");
    assert_eq!(health["opensearch"]["status"], "ok");
    assert_eq!(health["schema_ok"], true);

    // Never the `danswer_chunk*` wildcard the old client used.
    let index = health["index_name"].as_str().unwrap();
    assert!(!index.contains('*'), "index name is a wildcard: {index}");
    assert!(index.starts_with("danswer_chunk"));

    assert!(
        health["missing_indexes"].as_array().unwrap().is_empty(),
        "OVIS support indexes are missing; apply ops/onyx_indexes.sql: {}",
        health["missing_indexes"]
    );
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn the_default_listing_is_recent_populated_and_attributed() {
    let Some(base) = base_url() else {
        return skip("the_default_listing_is_recent_populated_and_attributed");
    };

    let page = get(&base, "/api/v1/pages?limit=50").await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 50, "a 1.65M-document corpus should fill a page");
    assert!(page["total"].as_i64().unwrap() > 1_000_000, "total looks wrong");

    // Newest first, for real data rather than a fixture.
    let timestamps: Vec<&str> = items
        .iter()
        .map(|i| i["updated_at"].as_str().unwrap())
        .collect();
    let mut sorted = timestamps.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(timestamps, sorted, "the listing is not newest-first");

    // Connector attribution comes from Postgres, so it must be populated without a
    // single OpenSearch call.
    let attributed = items
        .iter()
        .filter(|i| i["connector_name"].is_string())
        .count();
    assert!(
        attributed >= 45,
        "only {attributed}/50 rows carried connector attribution"
    );

    // Every row carries the field, even when its value is null.
    assert!(
        items.iter().all(|i| i.get("chunk_count").is_some()),
        "chunk_count must always be present, even when unknown"
    );

    // The newest documents on a live crawl legitimately have a *null* chunk_count:
    // Onyx records the count after indexing, so a page of just-crawled rows has
    // none yet. That is precisely why null is kept distinct from zero — reporting
    // these as 0-chunk stubs would be wrong. Ask for counted rows instead.
    let counted = get(&base, "/api/v1/pages?limit=20&chunk_min=1").await;
    let counted_items = counted["items"].as_array().unwrap();
    assert!(!counted_items.is_empty(), "no document reported any chunks");
    assert!(
        counted_items
            .iter()
            .all(|i| i["chunk_count"].as_i64().unwrap_or(0) >= 1),
        "chunk_min=1 returned a row with no chunks"
    );
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn cursor_paging_is_stable_over_live_data() {
    let Some(base) = base_url() else {
        return skip("cursor_paging_is_stable_over_live_data");
    };

    let mut seen: Vec<String> = Vec::new();
    let mut uri = "/api/v1/pages?limit=20".to_string();
    for _ in 0..5 {
        let page = get(&base, &uri).await;
        for item in page["items"].as_array().unwrap() {
            seen.push(item["id"].as_str().unwrap().to_string());
        }
        let Some(cursor) = page["next_cursor"].as_str() else { break };
        uri = format!(
            "/api/v1/pages?limit=20&cursor={}",
            urlencoding(cursor)
        );
    }

    assert_eq!(seen.len(), 100);
    let unique: std::collections::HashSet<&String> = seen.iter().collect();
    // A live crawl inserts rows underneath us, which is exactly why keyset paging
    // exists: it must not repeat a document even so.
    assert_eq!(
        unique.len(),
        seen.len(),
        "keyset paging repeated {} document(s) over live data",
        seen.len() - unique.len()
    );
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn connectors_show_the_real_paused_majority() {
    let Some(base) = base_url() else {
        return skip("connectors_show_the_real_paused_majority");
    };

    let connectors = get(&base, "/api/v1/connectors").await;
    let items = connectors.as_array().unwrap();
    assert!(items.len() > 300, "expected ~332 cc-pairs, got {}", items.len());

    // The C5 regression, on real data: the old query hardcoded `disabled = false`,
    // so the PAUSED majority was invisible.
    let paused = items.iter().filter(|c| c["status"] == "PAUSED").count();
    assert!(
        paused > 100,
        "only {paused} connectors reported PAUSED; status is not being read"
    );
    assert!(items.iter().any(|c| c["status"] == "ACTIVE"));

    // Document counts come from dcc, never from the unreliable
    // total_docs_indexed column.
    let biggest = items
        .iter()
        .map(|c| c["doc_count"].as_i64().unwrap_or(0))
        .max()
        .unwrap();
    assert!(biggest > 10_000, "the largest connector reported {biggest} docs");
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn search_returns_highlighted_hits_hydrated_from_postgres() {
    let Some(base) = base_url() else {
        return skip("search_returns_highlighted_hits_hydrated_from_postgres");
    };

    let results = get(&base, "/api/v1/search?q=the&limit=10").await;
    let items = results["items"].as_array().unwrap();
    assert!(!items.is_empty(), "a corpus of 10M chunks matched nothing");
    assert!(results["took_ms"].as_u64().unwrap() < 5_000);

    let hit = &items[0];
    assert!(hit["document_id"].is_string());
    assert!(
        hit["snippet"].is_string(),
        "hits should carry a highlighted snippet"
    );
    // Hydration means these come from Postgres, not from the chunk.
    assert!(
        items.iter().any(|h| h["chunk_count"].is_number()),
        "no hit was hydrated with its Postgres chunk count"
    );
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn semantic_search_admits_it_cannot_serve_this_index() {
    let Some(base) = base_url() else {
        return skip("semantic_search_admits_it_cannot_serve_this_index");
    };

    // The live index declares `embeddings.full_embedding` as a 768-dim knn_vector
    // but populates zero documents with it — Onyx writes `content_vector`, typed
    // as a plain float array, which cannot serve kNN. A semantic request must
    // fall back and say so rather than returning an empty result set.
    let results = get(&base, "/api/v1/search?q=inflation&mode=semantic&limit=5").await;
    assert_eq!(results["mode"], "semantic");
    let degraded = results["degraded"].as_str();
    if degraded.is_none() {
        // If a future re-index populates the kNN field, this becomes a real
        // semantic search — which is a pass, not a failure.
        assert!(
            !results["items"].as_array().unwrap().is_empty(),
            "semantic search reported no degradation and returned nothing"
        );
        return;
    }
    assert_eq!(degraded, Some("no_knn_field"));
    assert!(
        !results["items"].as_array().unwrap().is_empty(),
        "a degraded semantic search must still return keyword results"
    );
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn detail_chunks_and_vector_work_on_a_real_document() {
    let Some(base) = base_url() else {
        return skip("detail_chunks_and_vector_work_on_a_real_document");
    };

    let page = get(&base, "/api/v1/pages?limit=50&chunk_min=3").await;
    let document_id = page["items"][0]["id"].as_str().unwrap().to_string();
    let encoded = urlencoding(&document_id);

    let detail = get(&base, &format!("/api/v1/pages/{encoded}")).await;
    assert_eq!(detail["id"], document_id);
    assert_eq!(detail["pg_row"], true);
    assert!(detail["cc_pair_id"].is_number());

    let chunks = get(&base, &format!("/api/v1/pages/{encoded}/chunks?limit=3")).await;
    assert!(chunks["total_chunks"].as_i64().unwrap() >= 3);
    assert_eq!(chunks["embedding_dim"], 768);
    // No vectors in a bulk chunk read — that download was the old N+1's real cost.
    let rendered = chunks.to_string();
    assert!(!rendered.contains("content_vector"));
    assert!(!rendered.contains("full_embedding"));

    let vector = get(&base, &format!("/api/v1/pages/{encoded}/chunks/0/vector")).await;
    assert_eq!(vector["dim"], 768);
    assert_eq!(vector["vector"].as_array().unwrap().len(), 768);
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn stats_reflect_the_live_cluster_including_disk_headroom() {
    let Some(base) = base_url() else {
        return skip("stats_reflect_the_live_cluster_including_disk_headroom");
    };

    let overview = get(&base, "/api/v1/stats/overview").await;
    assert!(overview["documents"].as_i64().unwrap() > 1_000_000);
    assert!(overview["chunks"].as_i64().unwrap() > 1_000_000);
    assert!(overview["connectors"]["total"].as_i64().unwrap() > 300);
    assert_eq!(overview["embedding"]["dim"], 768);

    // Disk headroom is first-class: this index has tripped the flood-stage
    // read-only watermark before.
    let index = &overview["index"];
    assert!(index["size_bytes"].as_i64().unwrap() > 1_000_000_000);
    assert!(index["disk_used_pct"].as_f64().unwrap() > 0.0);
    assert!(index["read_only"].is_boolean());
    assert_eq!(index["cluster_status"], "green");
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn the_stream_delivers_its_full_contract_over_live_data() {
    let Some(base) = base_url() else {
        return skip("the_stream_delivers_its_full_contract_over_live_data");
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap();
    let mut request = client.get(format!("{base}/api/v1/pages/stream?limit=300"));
    if let Ok(token) = std::env::var("OVIS_API_TOKEN") {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let wire = request.send().await.unwrap().text().await.unwrap();

    assert_eq!(wire.matches("event: page").count(), 300);
    assert!(wire.contains("id: 0"));
    assert!(wire.contains("event: done"));
    assert!(!wire.contains("event: error"));

    let done: Value = serde_json::from_str(
        wire.split("event: done")
            .nth(1)
            .unwrap()
            .lines()
            .find(|l| l.starts_with("data: "))
            .unwrap()
            .trim_start_matches("data: "),
    )
    .unwrap();
    assert_eq!(done["total_matched"], 300);
}

#[tokio::test]
#[ignore = "needs a live OVIS server; set OVIS_SMOKE_URL"]
async fn errors_are_honest_over_the_live_deployment() {
    let Some(base) = base_url() else {
        return skip("errors_are_honest_over_the_live_deployment");
    };

    let client = reqwest::Client::new();
    let cases = [
        ("/api/v1/pages?sortt=x", 400, "BAD_REQUEST"),
        ("/api/v1/pages?sort=bogus", 400, "BAD_REQUEST"),
        ("/api/v1/pages?page=100000&limit=50", 400, "BAD_REQUEST"),
        (
            "/api/v1/pages/https%3A%2F%2Fexample.invalid%2Fnope",
            404,
            "NOT_FOUND",
        ),
        ("/api/v1/search", 400, "BAD_REQUEST"),
        ("/api/v1/nope", 404, "NOT_FOUND"),
    ];

    for (path, expected_status, expected_code) in cases {
        let mut request = client.get(format!("{base}{path}"));
        if let Ok(token) = std::env::var("OVIS_API_TOKEN") {
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, expected_status, "{path}: body {body}");
        assert_eq!(body["error"]["code"], expected_code, "{path}");
        let req_id = body["error"]["req_id"].as_str().unwrap();
        assert!(
            !req_id.is_empty() && req_id != "-",
            "{path}: the envelope must carry a correlatable req_id"
        );
    }
}

fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
