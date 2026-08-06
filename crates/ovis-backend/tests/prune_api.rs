//! Pruning contract + lifecycle tests, HTTP-level plus direct reaper cycles.
//!
//! Everything destructive here runs against the throwaway test database
//! (`scripts/test-db.sh up`), never a live deployment — this file is exactly
//! where the hard-delete paths are exercised for real. Skips itself when
//! `OVIS_TEST_DATABASE_URL` is unset.

use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use ovis_backend::config::ServerConfig;
use ovis_backend::services::{prune_reaper, prune_scan};
use ovis_backend::state::{AppState, BuildInfo, Caches, PruneHandle, RuntimeMeta};
use ovis_core::db::probe::SchemaProbe;
use ovis_core::search::{IndexCapabilities, OsClient};
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const INDEX: &str = "danswer_chunk_snowflake_arctic_embed_m";

const GUIDE_TEXT: &str = "This comprehensive operations guide describes how the homelab \
    search cluster is deployed, monitored and upgraded. It walks through Postgres tuning, \
    OpenSearch shard sizing, connector scheduling, embedding throughput, and the recovery \
    procedures used when the disk watermark trips or an index attempt stalls. Each section \
    closes with a checklist that the on-call operator is expected to follow before paging \
    anyone else about the incident at hand.";

const GERMAN_TEXT: &str = "Dieses Impressum enthält die gesetzlich vorgeschriebenen Angaben \
    über den Betreiber dieser Webseite sowie Hinweise zur Haftung für Inhalte und für Links \
    auf externe Seiten. Verantwortlich für den Inhalt nach § 55 Abs. 2 RStV ist die \
    Geschäftsführung der Beispiel GmbH mit Sitz in Berlin. Alle Rechte vorbehalten.";

/// Per-document chunk text served by the OpenSearch stand-in.
fn chunk_texts_for(doc_id: &str) -> Vec<String> {
    match doc_id {
        "https://paused.example/guide" => vec![GUIDE_TEXT.to_string()],
        "https://paused.example/guide-copy" => {
            vec![format!("{GUIDE_TEXT} Mirrored for the archive.")]
        }
        "https://paused.example/de/impressum" => {
            vec![GERMAN_TEXT.to_string(), GERMAN_TEXT.to_string()]
        }
        // An image indexed as a page: the crawl extracted the filename and
        // dimensions, nothing more.
        "https://paused.example/media/diagram.png" => {
            vec!["diagram.png (1200×800)".to_string()]
        }
        // A PDF with genuine prose of its own — proves PDFs are not treated
        // as assets, without accidentally near-duplicating the guide.
        "https://paused.example/reports/annual.pdf" => vec![
            "The annual report summarises procurement volumes, staffing changes and \
             capital expenditure across each regional office during the reporting year. \
             Figures are presented alongside the prior period so that variance can be \
             traced to specific programmes rather than to accounting adjustments made \
             at consolidation."
                .to_string(),
        ],
        // Navigation chrome: short unpunctuated lines, few stopwords.
        "https://paused.example/site/nav" => vec![
            "Home\nAbout\nContact\nProducts\nServices\nBlog\nCareers\nPress\nLegal\nHelp\n\
             Sitemap\nPrivacy\nTerms\nJobs\nNews"
                .to_string(),
        ],
        other => vec![
            format!(
                "chunk zero of {other} carrying enough distinct filler words that unrelated \
                 documents never shingle into each other during the near duplicate pass"
            ),
            format!("chunk one of {other} with a second body of unremarkable text"),
        ],
    }
}

/// Serialise the tests: they share one database and each re-seeds it.
static EXCLUSIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Harness {
    app: axum::Router,
    state: AppState,
    _os: MockServer,
    _lock: DbLock,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

/// The shared advisory-lock key, kept identical across every database-backed
/// suite in this workspace (see `ovis-core/tests/common/mod.rs`).
///
/// `cargo test` runs test binaries in parallel and they all point at one
/// database, so the in-process mutex above is not enough: another binary's
/// suite can be deleting the very documents this one is counting. The lock
/// lives on its own connection because a pooled connection would return to the
/// pool still holding it.
const DB_LOCK_KEY: i64 = 0x0715_0000_0000_0001;

struct DbLock(#[allow(dead_code)] Option<sqlx::PgConnection>);

impl DbLock {
    async fn acquire(dsn: &str) -> Self {
        use sqlx::Connection;
        match sqlx::PgConnection::connect(dsn).await {
            Ok(mut conn) => {
                if let Err(err) = sqlx::query("SELECT pg_advisory_lock($1)")
                    .bind(DB_LOCK_KEY)
                    .execute(&mut conn)
                    .await
                {
                    // Failing open would let suites run unserialized against one
                    // database and surface as unrelated assertion failures much
                    // later. Fail loudly here instead.
                    panic!("could not take the shared test-database lock: {err}");
                }
                Self(Some(conn))
            }
            Err(err) => {
                panic!("could not open a lock connection to the test database: {err}");
            }
        }
    }
}


/// OpenSearch stand-in. `read_only` controls what `{index}/_settings` reports,
/// which is what halts the reaper.
async fn mock_opensearch(read_only: bool) -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/_delete_by_query.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "deleted": 2 })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/_update_by_query.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "updated": 2 })))
        .mount(&server)
        .await;

    // Chunk fetches: per-document text so the content detectors (language,
    // near-duplicate, thin-words) have something real to chew on.
    Mock::given(method("POST"))
        .and(path_regex(r".*/_search$"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            if let Some(doc_id) = body["query"]["term"]["document_id"].as_str() {
                if body["search_after"][0].as_i64().is_some() {
                    return ResponseTemplate::new(200).set_body_json(json!({
                        "took": 1,
                        "hits": { "total": { "value": 2, "relation": "eq" }, "hits": [] }
                    }));
                }
                let chunks = chunk_texts_for(doc_id);
                let hits: Vec<Value> = chunks
                    .iter()
                    .enumerate()
                    .map(|(i, content)| {
                        json!({
                            "_id": format!("{doc_id}__{i}"),
                            "_source": {
                                "chunk_index": i,
                                "document_id": doc_id,
                                "content": content,
                                "blurb": "blurb",
                                "title": "Title",
                                "semantic_identifier": "Title",
                                "source_type": "web",
                                "hidden": false,
                            }
                        })
                    })
                    .collect();
                return ResponseTemplate::new(200).set_body_json(json!({
                    "took": 1,
                    "hits": { "total": { "value": chunks.len(), "relation": "eq" }, "hits": hits }
                }));
            }
            ResponseTemplate::new(200).set_body_json(json!({
                "took": 1,
                "hits": { "total": { "value": 0, "relation": "eq" }, "hits": [] }
            }))
        })
        .mount(&server)
        .await;

    // `_msearch`: the scan's content pass batches a whole page into one
    // request. Mounted before the `_search` matcher so the more specific path
    // wins.
    Mock::given(method("POST"))
        .and(path_regex(r".*/_msearch.*"))
        .respond_with(|request: &wiremock::Request| {
            let body = String::from_utf8_lossy(&request.body).to_string();
            let mut responses: Vec<Value> = Vec::new();
            // ndjson: alternating header and query lines.
            for line in body.lines().filter(|l| l.contains("document_id")) {
                let query: Value = serde_json::from_str(line).unwrap_or(Value::Null);
                let doc_id = query["query"]["term"]["document_id"].as_str().unwrap_or("");
                let chunks = chunk_texts_for(doc_id);
                let hits: Vec<Value> = chunks
                    .iter()
                    .enumerate()
                    .map(|(i, content)| {
                        json!({
                            "_id": format!("{doc_id}__{i}"),
                            "_source": {
                                "chunk_index": i,
                                "document_id": doc_id,
                                "content": content,
                                "blurb": "blurb",
                                "title": "Title",
                                "semantic_identifier": "Title",
                                "source_type": "web",
                                "hidden": false,
                            }
                        })
                    })
                    .collect();
                responses.push(json!({
                    "status": 200,
                    "hits": { "total": { "value": chunks.len(), "relation": "eq" }, "hits": hits }
                }));
            }
            ResponseTemplate::new(200).set_body_json(json!({ "responses": responses }))
        })
        .mount(&server)
        .await;

    let blocks = if read_only {
        json!({ "read_only_allow_delete": "true" })
    } else {
        json!({})
    };
    Mock::given(method("GET"))
        .and(path_regex(r".*/_settings$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            INDEX: { "settings": { "index": { "blocks": blocks } } }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "version": { "number": "3.0.0" } })),
        )
        .mount(&server)
        .await;

    server
}

async fn harness_with(
    read_only: bool,
    mut configure: impl FnMut(&mut ServerConfig),
) -> Option<Harness> {
    let guard = EXCLUSIVE.lock().await;
    let dsn = std::env::var("OVIS_TEST_DATABASE_URL").ok()?;
    if dsn.trim().is_empty() {
        return None;
    }

    // Cross-process lock before reseeding: another binary may be mid-test
    // against the rows about to be deleted.
    let lock = DbLock::acquire(&dsn).await;

    let os_server = mock_opensearch(read_only).await;

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
    let prune_enabled = ovis_core::db::prune::ensure_tables(&db).await;
    assert!(prune_enabled, "the test database must accept ovis DDL");
    assert!(
        ovis_core::db::trash::ensure_tables(&db).await,
        "the trash table must be creatable; the reaper refuses to delete without it"
    );
    // Exactly the conjunction `serve()` uses. Setting `llm_enabled` from the
    // provider tables alone left the annotation table uncreated while the flag
    // said the LLM subsystem was live, and every surface that reads a
    // generated title answered 500 — a divergence between the harness and the
    // startup path, not of the code under test.
    let llm_enabled = ovis_core::db::llm::ensure_tables(&db).await
        && ovis_core::db::annotation::ensure_tables(&db).await;
    assert!(
        ovis_core::db::pending_deletes::ensure_table(&db).await,
        "the retry queue table must be creatable"
    );

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
        pending_deletes_enabled: true,
        prune: PruneHandle::new(true, true),
        llm_enabled,
        metrics: None,
    };

    Some(Harness {
        app: ovis_backend::app(state.clone()),
        state,
        _os: os_server,
        _lock: lock,
        _guard: guard,
    })
}

async fn harness() -> Option<Harness> {
    harness_with(false, |_| {}).await
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
    sqlx::raw_sql(include_str!("../../../tests/fixtures/seed_prune.sql"))
        .execute(pool)
        .await
        .expect("prune seeding");

    for table in [
        "prune_audit",
        "prune_candidate",
        "prune_scan",
        "prune_exclusions",
        "prune_rules",
        "pending_index_deletes",
        // v2 tables. The trash in particular is keyed by document id, so a
        // snapshot left by an earlier test makes the next one's counts wrong
        // in a way that reads like a product bug.
        "trash_document",
        "pending_index_restores",
        "doc_profile",
        "dup_pair",
        "prune_policy",
        // Persisted MinHash signatures outlive a public reseed too, and the
        // near-duplicate tests assert on how many were written.
        "prune_minhash_band",
        "prune_minhash",
        // Generated titles are keyed by subject, so one left behind renames a
        // cluster in the next test's assertions. The provider tables leak the
        // same way and worse: a role assignment surviving from a dev session
        // makes "no model is configured" tests exercise a live endpoint.
        "llm_annotation",
        "llm_role",
        "llm_model",
        "llm_provider",
    ] {
        let _ = sqlx::query(&format!("DELETE FROM ovis.{table}"))
            .execute(pool)
            .await;
    }
    ovis_core::db::prune::ensure_tables(pool).await;
}

fn skip(test: &str) {
    eprintln!("SKIPPED {test}: set OVIS_TEST_DATABASE_URL (see `scripts/test-db.sh up`)");
}

struct Reply {
    status: StatusCode,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("expected JSON, got {:?}: {}", self.body, e))
    }
    fn error_code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
}

async fn send(app: &axum::Router, request: Request<Body>) -> Reply {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
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

async fn post_json(app: &axum::Router, uri: &str, body: Value) -> Reply {
    send(
        app,
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

/// Queue a scan over `scope` with `detectors`, drive it to completion, and
/// return the finished scan JSON.
async fn run_scan(h: &Harness, scope: Value, detectors: Value) -> Value {
    let queued = post_json(
        &h.app,
        "/api/v1/prune/scans",
        json!({ "scope": scope, "detectors": detectors }),
    )
    .await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{}", queued.body);
    let scan_id = queued.json()["id"].as_i64().unwrap();

    assert!(
        prune_scan::run_next_scan(&h.state).await,
        "a queued scan should be runnable"
    );

    let done = get(&h.app, &format!("/api/v1/prune/scans/{scan_id}")).await;
    assert_eq!(done.status, StatusCode::OK);
    done.json()
}

async fn doc_hidden(state: &AppState, id: &str) -> Option<bool> {
    sqlx::query_scalar("SELECT hidden FROM public.document WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
}

async fn candidate_ids_by_state(h: &Harness, state_name: &str) -> Vec<(i64, String)> {
    sqlx::query("SELECT id, document_id FROM ovis.prune_candidate WHERE state = $1 ORDER BY id")
        .bind(state_name)
        .fetch_all(&h.state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.get::<i64, _>("id"), r.get::<String, _>("document_id")))
        .collect()
}

// ===========================================================================
// M1 — read path + the two zero-content detectors
// ===========================================================================

#[tokio::test]
async fn scan_on_a_paused_connector_produces_reviewable_candidates_and_mutates_nothing() {
    let Some(h) = harness().await else {
        return skip("scan_on_a_paused_connector");
    };

    // Snapshot every document's hidden flag before the scan.
    let before: Vec<(String, bool)> =
        sqlx::query_as("SELECT id, hidden FROM public.document ORDER BY id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();

    let scan = run_scan(
        &h,
        json!({ "kind": "connectors", "connector_ids": [2] }),
        json!(["exact_duplicate", "thin"]),
    )
    .await;
    assert_eq!(scan["status"], "done", "{scan}");
    // 2 dup members (group of 3 minus the keeper) + 2 aged stubs on connector 2.
    assert_eq!(scan["stats"]["candidates_new"], 4, "{scan}");
    assert_eq!(scan["stats"]["dup_groups"], 1);
    assert_eq!(scan["stats"]["dup_members"], 2);
    assert_eq!(scan["stats"]["stub_hits"], 2);
    assert!(scan["config_hash"].as_str().unwrap().len() >= 16);

    // Reviewable via the API, with per-document reasons.
    let list = get(&h.app, "/api/v1/prune/candidates").await;
    assert_eq!(list.status, StatusCode::OK);
    let body = list.json();
    assert_eq!(body["total"], 4);
    for item in body["items"].as_array().unwrap() {
        assert_eq!(item["state"], "candidate");
        assert!(!item["reasons"].as_array().unwrap().is_empty());
        assert_eq!(item["recrawl_risk"], false, "connector 2 is PAUSED");
        assert_eq!(item["connector_id"], 2);
    }

    // The keeper (shortest URL) is not flagged; members carry the evidence.
    let flagged: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["document_id"].as_str().unwrap())
        .collect();
    assert!(!flagged.contains(&"https://paused.example/dup"));
    assert!(flagged.contains(&"https://paused.example/dup?utm_source=feed"));
    assert!(flagged.contains(&"https://paused.example/dup/print/view"));

    let dup = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["document_id"] == "https://paused.example/dup/print/view")
        .unwrap();
    let reason = &dup["reasons"][0];
    assert_eq!(reason["detector"], "duplicate");
    assert_eq!(reason["code"], "exact_duplicate_of");
    assert_eq!(reason["confidence"], 1.0);
    assert_eq!(reason["evidence"]["kept"], "https://paused.example/dup");
    assert_eq!(reason["evidence"]["group_size"], 2 + 1);

    // Detail hydrates both sides of the pair.
    let detail = get(
        &h.app,
        &format!("/api/v1/prune/candidates/{}", dup["id"].as_i64().unwrap()),
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK);
    let detail = detail.json();
    assert_eq!(detail["pair"]["kept_id"], "https://paused.example/dup");
    assert_eq!(detail["pair"]["kept"]["semantic_id"], "Dup Canonical");
    assert_eq!(detail["pair"]["similarity"], 1.0);

    // A scan is a preview: zero document mutations.
    let after: Vec<(String, bool)> =
        sqlx::query_as("SELECT id, hidden FROM public.document ORDER BY id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    assert_eq!(before, after, "a scan must not touch documents");

    // And the audit shows only scan activity — no lifecycle actions.
    let audit = get(&h.app, "/api/v1/prune/audit").await.json();
    for row in audit["items"].as_array().unwrap() {
        let action = row["action"].as_str().unwrap();
        assert!(
            action.starts_with("scan_") || action == "candidates_closed",
            "unexpected audit action after a dry scan: {action}"
        );
    }
}

#[tokio::test]
async fn null_chunk_count_is_never_flagged_and_fresh_stubs_are_age_gated() {
    let Some(h) = harness().await else {
        return skip("null_chunk_count_never_flagged");
    };

    let scan = run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    assert_eq!(scan["status"], "done");
    // Scope total counts every document, and examined walked them all.
    assert_eq!(scan["examined"], scan["total"], "{scan}");

    let body = get(&h.app, "/api/v1/prune/candidates?detector=thin&limit=100")
        .await
        .json();
    let flagged: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["document_id"].as_str().unwrap())
        .collect();

    assert!(flagged.contains(&"https://paused.example/old-stub"));
    assert!(flagged.contains(&"https://example.com/active-stub"));
    assert!(flagged.contains(&"https://paused.example/already-hidden-stub"));
    assert!(
        !flagged.contains(&"https://example.com/uncounted"),
        "chunk_count NULL means 'not counted yet', never 'empty'"
    );
    assert!(
        !flagged.contains(&"https://example.com/stub"),
        "a 5-day-old stub is inside the 7-day age gate"
    );

    // The ACTIVE pair's stub carries the recrawl warning.
    let active = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["document_id"] == "https://example.com/active-stub")
        .unwrap();
    assert_eq!(active["recrawl_risk"], true);
    assert_eq!(active["chunk_count"], 0);
}

#[tokio::test]
async fn scan_validation_fails_loudly() {
    let Some(h) = harness().await else {
        return skip("scan_validation");
    };

    // Unknown detector.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/scans",
        json!({ "scope": { "kind": "all" }, "detectors": ["dedupe"] }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.body.contains("exact_duplicate"), "{}", reply.body);

    // Empty detector list.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/scans",
        json!({ "scope": { "kind": "all" }, "detectors": [] }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);

    // Bad scope.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/scans",
        json!({ "scope": { "kind": "connectors" }, "detectors": ["thin"] }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);

    // Unknown query parameter on candidates.
    let reply = get(&h.app, "/api/v1/prune/candidates?min_confidance=0.5").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.error_code(), "BAD_REQUEST");

    // Only one scan owns the queue at a time.
    let first = post_json(
        &h.app,
        "/api/v1/prune/scans",
        json!({ "scope": { "kind": "all" }, "detectors": ["thin"] }),
    )
    .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    let second = post_json(
        &h.app,
        "/api/v1/prune/scans",
        json!({ "scope": { "kind": "all" }, "detectors": ["thin"] }),
    )
    .await;
    assert_eq!(second.status, StatusCode::CONFLICT);
    assert_eq!(second.error_code(), "CONFLICT");
}

#[tokio::test]
async fn a_rescan_updates_open_candidates_and_closes_resolved_ones() {
    let Some(h) = harness().await else {
        return skip("rescan_updates_and_closes");
    };

    let first = run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    assert_eq!(first["stats"]["candidates_new"], 3);
    let open_before = candidate_ids_by_state(&h, "candidate").await;

    // The old stub gets chunked in the meantime — it is no longer a stub.
    sqlx::query("UPDATE public.document SET chunk_count = 5 WHERE id = 'https://paused.example/old-stub'")
        .execute(&h.state.db)
        .await
        .unwrap();

    let second = run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    assert_eq!(second["status"], "done");
    assert_eq!(
        second["stats"]["candidates_new"], 0,
        "a re-scan updates, never duplicates: {second}"
    );
    assert_eq!(second["stats"]["candidates_updated"], 2);
    assert_eq!(second["stats"]["candidates_closed"], 1);

    let open_after = candidate_ids_by_state(&h, "candidate").await;
    assert_eq!(open_after.len(), 2);
    // Surviving candidates kept their ids (updated in place).
    for (id, doc) in &open_after {
        assert!(open_before.contains(&(*id, doc.clone())));
    }

    let closed: (String, String) = sqlx::query_as(
        "SELECT state, resolved_reason FROM ovis.prune_candidate \
         WHERE document_id = 'https://paused.example/old-stub'",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(closed.0, "dismissed");
    assert_eq!(closed.1, "no_longer_matches");
}

// ===========================================================================
// M2 — the lifecycle spine
// ===========================================================================

#[tokio::test]
async fn stage_requires_a_matching_confirm_count_and_is_exactly_reversible() {
    let Some(h) = harness().await else {
        return skip("stage_confirm_count_reversible");
    };

    run_scan(
        &h,
        json!({ "kind": "connectors", "connector_ids": [2] }),
        json!(["thin"]),
    )
    .await;

    // Missing confirm_count → 400 that names the requirement.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "filter": {} }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.body.contains("confirm_count"), "{}", reply.body);

    // Wrong confirm_count → 409 carrying the fresh count, nothing changed.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "filter": {}, "confirm_count": 1 }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(
        reply.body.contains("confirm_count=2"),
        "the 409 must carry the fresh count: {}",
        reply.body
    );
    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/old-stub").await,
        Some(false),
        "a drifted confirm_count must change nothing"
    );

    // Byte-exactness baseline: the whole document row before staging.
    let before: Value = sqlx::query_scalar(
        "SELECT row_to_json(d)::jsonb FROM public.document d WHERE id = 'https://paused.example/old-stub'",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();

    // Correct count → both stubs staged (hidden), grace running.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "filter": {}, "confirm_count": 2 }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = reply.json();
    assert_eq!(body["changed"], 2);
    assert_eq!(body["state"], "staged");
    assert_eq!(body["boost_hidden_via"], "direct_sql");
    let expires = body["stage_expires_at"].as_str().unwrap();
    let expires: chrono::DateTime<chrono::Utc> = expires.parse().unwrap();
    let days = (expires - chrono::Utc::now()).num_hours();
    assert!((167..=169).contains(&days), "grace should be ~7 days, got {days}h");

    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/old-stub").await,
        Some(true)
    );
    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/already-hidden-stub").await,
        Some(true)
    );

    // prev_hidden recorded per document.
    let prev: Vec<(String, bool)> = sqlx::query_as(
        "SELECT document_id, prev_hidden FROM ovis.prune_candidate WHERE state = 'staged' ORDER BY document_id",
    )
    .fetch_all(&h.state.db)
    .await
    .unwrap();
    assert_eq!(
        prev,
        vec![
            ("https://paused.example/already-hidden-stub".to_string(), true),
            ("https://paused.example/old-stub".to_string(), false),
        ]
    );

    // Restore: no confirm needed (the safe direction), exact by construction.
    let reply = post_json(&h.app, "/api/v1/prune/candidates/restore", json!({ "filter": {} })).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.json()["changed"], 2);

    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/old-stub").await,
        Some(false),
        "restore returns a visible document to visible"
    );
    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/already-hidden-stub").await,
        Some(true),
        "a document hidden before staging returns to hidden-but-unstaged"
    );

    let after: Value = sqlx::query_scalar(
        "SELECT row_to_json(d)::jsonb FROM public.document d WHERE id = 'https://paused.example/old-stub'",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(before, after, "stage → restore must be byte-exact");

    // Audit trail: staged and restored rows for both documents.
    let audit = get(&h.app, "/api/v1/prune/audit?action=staged").await.json();
    assert_eq!(audit["total"], 2);
    let audit = get(&h.app, "/api/v1/prune/audit?action=restored").await.json();
    assert_eq!(audit["total"], 2);
}

#[tokio::test]
async fn dismiss_with_exclusion_prevents_reflagging() {
    let Some(h) = harness().await else {
        return skip("dismiss_exclusion");
    };

    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    let open = candidate_ids_by_state(&h, "candidate").await;
    let (target_id, target_doc) = open
        .iter()
        .find(|(_, d)| d == "https://paused.example/old-stub")
        .cloned()
        .unwrap();

    let reply = post_json(
        &h.app,
        "/api/v1/prune/candidates/dismiss",
        json!({ "ids": [target_id], "exclude_future": true }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);

    let exclusions = get(&h.app, "/api/v1/prune/exclusions").await.json();
    assert_eq!(exclusions["total"], 1);
    assert_eq!(exclusions["items"][0]["document_id"], target_doc);
    assert_eq!(exclusions["items"][0]["reason"], "user_excluded");

    // A re-scan skips it and says so.
    let rescan = run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    assert!(
        rescan["stats"]["excluded_skipped"].as_i64().unwrap() >= 1,
        "{rescan}"
    );
    let still_dismissed: String = sqlx::query_scalar(
        "SELECT state FROM ovis.prune_candidate WHERE document_id = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(&target_doc)
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(still_dismissed, "dismissed");
}

#[tokio::test]
async fn schedule_delete_stages_candidates_with_full_grace_and_expedites_staged() {
    let Some(h) = harness().await else {
        return skip("schedule_delete_stages_then_expedites");
    };

    run_scan(
        &h,
        json!({ "kind": "connectors", "connector_ids": [2] }),
        json!(["exact_duplicate"]),
    )
    .await;
    let open = candidate_ids_by_state(&h, "candidate").await;
    assert_eq!(open.len(), 2);

    // Scheduling deletion of *candidates* stages them — full grace, hidden,
    // restorable. There is no way to skip the waiting room.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "filter": {}, "confirm_count": 2, "remember": true }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = reply.json();
    assert_eq!(body["state"], "staged");
    assert_eq!(body["changed"], 2);

    let staged: Vec<(String, bool)> = sqlx::query_as(
        "SELECT document_id, remember FROM ovis.prune_candidate WHERE state = 'staged' \
         AND stage_expires_at > now() + interval '6 days'",
    )
    .fetch_all(&h.state.db)
    .await
    .unwrap();
    assert_eq!(staged.len(), 2, "candidates get the full grace period");
    assert!(staged.iter().all(|(_, remember)| *remember));

    // A second schedule-delete on the now-staged rows brings the deadline to
    // now — the "delete sooner" direction — still reaper-executed.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "filter": { "state": "staged" }, "confirm_count": 2 }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let due: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ovis.prune_candidate WHERE state = 'staged' AND stage_expires_at <= now()",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(due, 2, "expedited rows are due immediately");
}

#[tokio::test]
async fn the_reaper_deletes_due_documents_through_the_full_cascade() {
    let Some(h) = harness().await else {
        return skip("reaper_deletes_due");
    };

    run_scan(
        &h,
        json!({ "kind": "connectors", "connector_ids": [2] }),
        json!(["exact_duplicate"]),
    )
    .await;

    // Park-sentinel + Onyx-config baselines, asserted untouched afterwards.
    let sentinel_before: String =
        sqlx::query_scalar("SELECT error_msg FROM public.index_attempt WHERE id = 5")
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    let search_settings_before: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, status FROM public.search_settings ORDER BY id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    let connector_cfg_before: Value = sqlx::query_scalar(
        "SELECT connector_specific_config FROM public.connector WHERE id = 2",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();

    // Stage + expedite both duplicates, remembering them.
    post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "filter": {}, "confirm_count": 2, "remember": true }),
    )
    .await;
    post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "filter": { "state": "staged" }, "confirm_count": 2 }),
    )
    .await;

    // Also stage one *without* expediting: it must survive the cycle.
    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    let open = candidate_ids_by_state(&h, "candidate").await;
    let (keep_id, keep_doc) = open
        .iter()
        .find(|(_, d)| d == "https://paused.example/old-stub")
        .cloned()
        .unwrap();
    post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "ids": [keep_id], "confirm_count": 1 }),
    )
    .await;

    let report = prune_reaper::run_cycle(&h.state).await.expect("cycle");
    assert_eq!(report.deleted, 2, "only the due documents are deleted");
    assert!(!report.halted);

    // The due documents are gone from Postgres, FK children included.
    for doc in [
        "https://paused.example/dup?utm_source=feed",
        "https://paused.example/dup/print/view",
    ] {
        assert_eq!(doc_hidden(&h.state, doc).await, None, "{doc} should be deleted");
        let dcc: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.document_by_connector_credential_pair WHERE id = $1",
        )
        .bind(doc)
        .fetch_one(&h.state.db)
        .await
        .unwrap();
        assert_eq!(dcc, 0);
        let tags: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public.document__tag WHERE document_id = $1")
                .bind(doc)
                .fetch_one(&h.state.db)
                .await
                .unwrap();
        assert_eq!(tags, 0);
    }
    // The keeper survives.
    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/dup").await,
        Some(false)
    );
    // The staged-but-not-due document survives, still staged.
    assert_eq!(doc_hidden(&h.state, &keep_doc).await, Some(true));

    // Candidate rows carry the honest outcome.
    let outcomes: Vec<(String, Value)> = sqlx::query_as(
        "SELECT document_id, delete_outcome FROM ovis.prune_candidate WHERE state = 'deleted' ORDER BY document_id",
    )
    .fetch_all(&h.state.db)
    .await
    .unwrap();
    assert_eq!(outcomes.len(), 2);
    for (_, outcome) in &outcomes {
        assert_eq!(outcome["chunks_deleted"], 2, "{outcome}");
        assert_eq!(outcome["index_cleanup_pending"], false);
    }

    // remember=true recorded exclusions.
    let exclusions = get(&h.app, "/api/v1/prune/exclusions").await.json();
    assert_eq!(exclusions["total"], 2);
    for item in exclusions["items"].as_array().unwrap() {
        assert_eq!(item["reason"], "deleted_with_remember");
    }

    // Audit: two deleted rows with outcomes.
    let audit = get(&h.app, "/api/v1/prune/audit?action=deleted").await.json();
    assert_eq!(audit["total"], 2);

    // The landmines are untouched.
    let sentinel_after: String =
        sqlx::query_scalar("SELECT error_msg FROM public.index_attempt WHERE id = 5")
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(sentinel_before, sentinel_after, "park sentinel must survive");
    assert_eq!(sentinel_after, "first-pass already complete");
    let search_settings_after: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, status FROM public.search_settings ORDER BY id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    assert_eq!(search_settings_before, search_settings_after);
    let connector_cfg_after: Value = sqlx::query_scalar(
        "SELECT connector_specific_config FROM public.connector WHERE id = 2",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(connector_cfg_before, connector_cfg_after);
}

#[tokio::test]
async fn the_reaper_defers_documents_on_pairs_that_are_indexing() {
    let Some(h) = harness().await else {
        return skip("reaper_defers_on_indexing");
    };

    // active-stub belongs to cc-pair 1, which has a fresh IN_PROGRESS attempt
    // in the fixture.
    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    let open = candidate_ids_by_state(&h, "candidate").await;
    let (id, _) = open
        .iter()
        .find(|(_, d)| d == "https://example.com/active-stub")
        .cloned()
        .unwrap();

    post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "ids": [id], "confirm_count": 1 }),
    )
    .await;
    post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "ids": [id], "confirm_count": 1 }),
    )
    .await;

    let report = prune_reaper::run_cycle(&h.state).await.expect("cycle");
    assert_eq!(report.deleted, 0);
    assert_eq!(report.deferred_indexing, 1);

    assert_eq!(
        doc_hidden(&h.state, "https://example.com/active-stub").await,
        Some(true),
        "a deferred document stays staged (hidden, intact)"
    );

    let status = get(&h.app, "/api/v1/prune/status").await.json();
    assert_eq!(status["reaper"]["deferred"], 1);
    assert_eq!(status["reaper"]["deferred_reason"], "indexing_in_progress");

    let audit = get(&h.app, "/api/v1/prune/audit?action=deferred").await.json();
    assert!(audit["total"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn the_reaper_halts_when_the_index_is_read_only() {
    let Some(h) = harness_with(true, |_| {}).await else {
        return skip("reaper_halts_read_only");
    };

    run_scan(
        &h,
        json!({ "kind": "connectors", "connector_ids": [2] }),
        json!(["exact_duplicate"]),
    )
    .await;
    post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "filter": {}, "confirm_count": 2 }),
    )
    .await;
    post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "filter": { "state": "staged" }, "confirm_count": 2 }),
    )
    .await;

    let report = prune_reaper::run_cycle(&h.state).await.expect("cycle");
    assert!(report.halted);
    assert_eq!(report.deleted, 0);

    // Nothing was deleted; the due rows are still staged and restorable.
    assert!(doc_hidden(&h.state, "https://paused.example/dup?utm_source=feed")
        .await
        .is_some());
    let staged: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ovis.prune_candidate WHERE state = 'staged'")
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(staged, 2);

    let status = get(&h.app, "/api/v1/prune/status").await.json();
    assert_eq!(status["reaper"]["halted"], true);
    assert_eq!(status["reaper"]["halted_reason"], "index_read_only");

    let audit = get(&h.app, "/api/v1/prune/audit?action=halted").await.json();
    assert_eq!(audit["total"], 1);
}

#[tokio::test]
async fn a_crashed_reaper_run_recovers_without_double_deleting() {
    let Some(h) = harness().await else {
        return skip("reaper_crash_recovery");
    };

    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    let open = candidate_ids_by_state(&h, "candidate").await;
    let (intact_id, intact_doc) = open
        .iter()
        .find(|(_, d)| d == "https://paused.example/old-stub")
        .cloned()
        .unwrap();
    let (gone_id, gone_doc) = open
        .iter()
        .find(|(_, d)| d == "https://paused.example/already-hidden-stub")
        .cloned()
        .unwrap();

    // Simulate a crash mid-batch: both rows stuck in `deleting`, stale.
    // The first document is intact (cascade never committed); the second's
    // Postgres delete committed before the crash.
    for id in [intact_id, gone_id] {
        sqlx::query(
            "UPDATE ovis.prune_candidate SET state = 'deleting', \
             stage_expires_at = now() + interval '3 days', \
             staged_at = now(), prev_hidden = false, remember = true, \
             updated_at = now() - interval '1 hour' WHERE id = $1",
        )
        .bind(id)
        .execute(&h.state.db)
        .await
        .unwrap();
    }
    sqlx::query("DELETE FROM public.document_by_connector_credential_pair WHERE id = $1")
        .bind(&gone_doc)
        .execute(&h.state.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM public.document WHERE id = $1")
        .bind(&gone_doc)
        .execute(&h.state.db)
        .await
        .unwrap();

    let report = prune_reaper::run_cycle(&h.state).await.expect("cycle");
    assert_eq!(report.recovered, 2);
    assert_eq!(report.deleted, 0, "recovery is not deletion");

    // The intact document went back to staged — no second cascade ran.
    let intact_state: String =
        sqlx::query_scalar("SELECT state FROM ovis.prune_candidate WHERE id = $1")
            .bind(intact_id)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(intact_state, "staged");
    assert_eq!(doc_hidden(&h.state, &intact_doc).await, Some(false));

    // The already-gone document is closed honestly, with cleanup queued.
    let (gone_state, outcome): (String, Value) = sqlx::query_as(
        "SELECT state, delete_outcome FROM ovis.prune_candidate WHERE id = $1",
    )
    .bind(gone_id)
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(gone_state, "deleted");
    assert_eq!(outcome["recovered_after_crash"], true);
    assert_eq!(outcome["index_cleanup_pending"], true);
    assert!(outcome["chunks_deleted"].is_null(), "unknown is null, not 0");

    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ovis.pending_index_deletes WHERE document_id = $1",
    )
    .bind(&gone_doc)
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(queued, 1, "index cleanup must be queued for the half-deleted doc");
}

#[tokio::test]
async fn the_reaper_closes_documents_that_vanished_before_their_grace_ended() {
    let Some(h) = harness().await else {
        return skip("reaper_already_gone");
    };

    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    let open = candidate_ids_by_state(&h, "candidate").await;
    let (id, doc) = open
        .iter()
        .find(|(_, d)| d == "https://paused.example/old-stub")
        .cloned()
        .unwrap();
    post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "ids": [id], "confirm_count": 1, "remember": true }),
    )
    .await;
    post_json(
        &h.app,
        "/api/v1/prune/candidates/schedule-delete",
        json!({ "ids": [id], "confirm_count": 1 }),
    )
    .await;

    // A connector delete (or manual delete) removes the document while it
    // waits — the reaper must close the lifecycle honestly, not error forever.
    sqlx::query("DELETE FROM public.document_by_connector_credential_pair WHERE id = $1")
        .bind(&doc)
        .execute(&h.state.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM public.document WHERE id = $1")
        .bind(&doc)
        .execute(&h.state.db)
        .await
        .unwrap();

    let report = prune_reaper::run_cycle(&h.state).await.expect("cycle");
    assert_eq!(report.deleted, 1, "closed as deleted, not stuck");

    let (state_name, outcome): (String, Value) =
        sqlx::query_as("SELECT state, delete_outcome FROM ovis.prune_candidate WHERE id = $1")
            .bind(id)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(state_name, "deleted");
    assert_eq!(outcome["already_gone"], true);
    assert_eq!(outcome["index_cleanup_pending"], true, "chunks may remain; queued");
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ovis.pending_index_deletes WHERE document_id = $1")
            .bind(&doc)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(queued, 1);
    // remember still records the exclusion — if it is recrawled, it re-stages.
    let excluded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ovis.prune_exclusions WHERE document_id = $1")
            .bind(&doc)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(excluded, 1);
}

#[tokio::test]
async fn restore_and_reaper_races_settle_by_state() {
    let Some(h) = harness().await else {
        return skip("restore_reaper_race");
    };

    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    let open = candidate_ids_by_state(&h, "candidate").await;
    let (id, _) = open
        .iter()
        .find(|(_, d)| d == "https://paused.example/old-stub")
        .cloned()
        .unwrap();
    post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "ids": [id], "confirm_count": 1 }),
    )
    .await;

    // The reaper claims the row (as if mid-cascade)…
    sqlx::query(
        "UPDATE ovis.prune_candidate SET state = 'deleting', stage_expires_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&h.state.db)
    .await
    .unwrap();

    // …so a concurrent restore reports the conflict per-id instead of lying.
    let reply = post_json(
        &h.app,
        "/api/v1/prune/candidates/restore",
        json!({ "ids": [id] }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::MULTI_STATUS, "{}", reply.body);
    let body = reply.json();
    assert_eq!(body["success"], false);
    assert_eq!(body["failed"][0]["code"], "WRONG_STATE");
}

#[tokio::test]
async fn recrawled_remembered_documents_are_restaged_never_deleted() {
    let Some(h) = harness().await else {
        return skip("recrawl_restage");
    };

    // A document that was pruned with remember=true earlier… (exclusion row +
    // closed candidate history)
    sqlx::query(
        "INSERT INTO ovis.prune_exclusions (document_id, reason, note) \
         VALUES ('https://paused.example/old-stub', 'deleted_with_remember', 'thin/chunkless_stub')",
    )
    .execute(&h.state.db)
    .await
    .unwrap();
    // …and the crawler has brought it back (the document row exists in the
    // fixture). The reaper's re-prune pass must stage it — not delete it.
    let report = prune_reaper::run_cycle(&h.state).await.expect("cycle");
    assert_eq!(report.restaged, 1);
    assert_eq!(report.deleted, 0);

    let (state_name, staged_by, remember): (String, String, bool) = sqlx::query_as(
        "SELECT state, staged_by, remember FROM ovis.prune_candidate \
         WHERE document_id = 'https://paused.example/old-stub'",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(state_name, "staged", "re-staged, never re-deleted");
    assert_eq!(staged_by, "reaper");
    assert!(remember);

    // Hidden, data intact, full grace ahead.
    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/old-stub").await,
        Some(true)
    );
    let due_now: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ovis.prune_candidate WHERE state = 'staged' AND stage_expires_at <= now()",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(due_now, 0, "the re-staged document gets the full grace period");

    // The reason is specific, and user_excluded documents are NOT re-staged.
    let reasons: Value = sqlx::query_scalar(
        "SELECT reasons FROM ovis.prune_candidate WHERE document_id = 'https://paused.example/old-stub'",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(reasons[0]["code"], "recrawled_after_prune");

    let audit = get(&h.app, "/api/v1/prune/audit?action=restaged_recrawled")
        .await
        .json();
    assert_eq!(audit["total"], 1);

    // A second cycle is idempotent: the open row blocks a duplicate.
    let report = prune_reaper::run_cycle(&h.state).await.expect("cycle");
    assert_eq!(report.restaged, 0);
}

#[tokio::test]
async fn prune_status_reports_real_counts_and_limits() {
    let Some(h) = harness().await else {
        return skip("prune_status_counts");
    };

    let status = get(&h.app, "/api/v1/prune/status").await;
    assert_eq!(status.status, StatusCode::OK);
    let body = status.json();
    assert_eq!(body["candidates"], 0);
    assert_eq!(body["staged"], 0);
    assert_eq!(body["limits"]["grace_days"], 7);
    assert_eq!(body["limits"]["big_batch"], 500);
    assert_eq!(body["limits"]["max_docs_per_hour"], 2000);
    assert_eq!(body["reaper"]["halted"], false);

    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    let body = get(&h.app, "/api/v1/prune/status").await.json();
    assert_eq!(body["candidates"], 3);
    assert_eq!(body["staged"], 0);
    assert!(body["soonest_expiry"].is_null());
}

// ===========================================================================
// M3 — heavy detectors, rules, config round-trip, resume
// ===========================================================================

#[tokio::test]
async fn near_duplicate_scan_finds_the_pair_with_persisted_signatures() {
    let Some(h) = harness().await else {
        return skip("near_duplicate_scan");
    };

    let scan = run_scan(
        &h,
        json!({ "kind": "connectors", "connector_ids": [2] }),
        json!(["near_duplicate"]),
    )
    .await;
    assert_eq!(scan["status"], "done", "{scan}");
    assert!(
        scan["stats"]["signatures_written"].as_i64().unwrap_or(-1) >= 3,
        "{scan}"
    );
    assert!(
        scan["stats"]["near_hits"].as_i64().unwrap_or(-1) >= 1,
        "{scan}"
    );

    let body = get(&h.app, "/api/v1/prune/candidates?detector=duplicate&limit=100")
        .await
        .json();
    let copy = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["document_id"] == "https://paused.example/guide-copy")
        .expect("the near-copy is flagged");
    let reason = &copy["reasons"][0];
    assert_eq!(reason["code"], "near_duplicate_of");
    assert_eq!(reason["evidence"]["kept"], "https://paused.example/guide");
    let sim = reason["confidence"].as_f64().unwrap();
    assert!(sim >= 0.8, "estimated Jaccard should be high, got {sim}");

    // The keeper is never flagged.
    assert!(body["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["document_id"] != "https://paused.example/guide"));

    // Unrelated documents are not flagged as near-duplicates.
    assert!(body["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["document_id"] != "https://paused.example/de/impressum"));

    // A re-scan reuses unchanged signatures instead of refetching content.
    let rescan = run_scan(
        &h,
        json!({ "kind": "connectors", "connector_ids": [2] }),
        json!(["near_duplicate"]),
    )
    .await;
    assert!(
        rescan["stats"]["signatures_reused"].as_i64().unwrap() >= 3,
        "{rescan}"
    );
    assert_eq!(rescan["stats"]["candidates_new"], 0, "updated, not duplicated");
}

#[tokio::test]
async fn language_scan_flags_disallowed_languages_when_enabled() {
    let Some(h) = harness().await else {
        return skip("language_scan");
    };

    // Language detection ships OFF; a scan with it merely requested but not
    // enabled flags nothing.
    let scan = run_scan(
        &h,
        json!({ "kind": "connectors", "connector_ids": [2] }),
        json!(["language"]),
    )
    .await;
    assert_eq!(scan["stats"]["lang_hits"], 0, "ships off: {scan}");

    // Enable it via a per-scan override (the same shape stored configs use).
    let queued = post_json(
        &h.app,
        "/api/v1/prune/scans",
        json!({
            "scope": { "kind": "connectors", "connector_ids": [2] },
            "detectors": ["language"],
            "config_overrides": { "language": { "enabled": true, "allowed": ["en"] } }
        }),
    )
    .await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{}", queued.body);
    prune_scan::run_next_scan(&h.state).await;

    let body = get(&h.app, "/api/v1/prune/candidates?detector=language&limit=100")
        .await
        .json();
    assert_eq!(body["total"], 1, "only the German page: {body}");
    let item = &body["items"][0];
    assert_eq!(item["document_id"], "https://paused.example/de/impressum");
    let reason = &item["reasons"][0];
    assert_eq!(reason["code"], "lang_not_allowed");
    assert_eq!(reason["evidence"]["detected"], "deu");
    assert!(reason["confidence"].as_f64().unwrap() >= 0.85);
}

#[tokio::test]
async fn url_rules_flag_matches_and_carry_the_rule_name() {
    let Some(h) = harness().await else {
        return skip("url_rule_scan");
    };

    // Enable the starter tracking-params rule.
    let rules = get(&h.app, "/api/v1/prune/rules").await.json();
    let rule_id = rules
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "tracking-params")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let reply = send(
        &h.app,
        Request::builder()
            .method("PATCH")
            .uri(format!("/api/v1/prune/rules/{rule_id}"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "enabled": true }).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);

    let scan = run_scan(&h, json!({ "kind": "all" }), json!(["url_rule"])).await;
    // Both tracking-parameter URLs in the fixture match: the utm-tagged
    // duplicate and the utm-tagged URL variant.
    assert_eq!(scan["stats"]["url_rule_hits"], 2, "{scan}");

    let body = get(&h.app, "/api/v1/prune/candidates?detector=url_rule")
        .await
        .json();
    assert_eq!(body["total"], 2);
    let flagged: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["document_id"].as_str().unwrap())
        .collect();
    assert!(flagged.contains(&"https://paused.example/dup?utm_source=feed"), "{flagged:?}");
    let item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["document_id"] == "https://paused.example/dup?utm_source=feed")
        .unwrap();
    assert_eq!(item["reasons"][0]["code"], "tracking-params");
    assert_eq!(item["reasons"][0]["confidence"], 0.95);
}

#[tokio::test]
async fn config_export_import_round_trips_and_takes_effect() {
    let Some(h) = harness().await else {
        return skip("config_round_trip");
    };

    let exported = get(&h.app, "/api/v1/prune/config").await;
    assert_eq!(exported.status, StatusCode::OK);
    assert!(exported.body.contains("language:"), "{}", exported.body);
    assert!(exported.body.contains("enabled: false"));

    // Import a config with language detection on.
    let modified = exported.body.replace(
        "language:\n  enabled: false",
        "language:\n  enabled: true",
    );
    assert_ne!(modified, exported.body, "the fixture edit must apply");
    let reply = send(
        &h.app,
        Request::builder()
            .method("PUT")
            .uri("/api/v1/prune/config")
            .header("content-type", "application/yaml")
            .body(Body::from(modified))
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.json()["kind"], "detector_config");
    assert_eq!(reply.json()["enabled"], true);

    let re_exported = get(&h.app, "/api/v1/prune/config").await;
    assert!(
        re_exported.body.contains("enabled: true"),
        "the import must take effect: {}",
        re_exported.body
    );

    // Garbage in → loud 400, nothing stored.
    let reply = send(
        &h.app,
        Request::builder()
            .method("PUT")
            .uri("/api/v1/prune/config")
            .header("content-type", "application/yaml")
            .body(Body::from("langauge:\n  enabled: true\n"))
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
}

#[tokio::test]
async fn a_running_scan_resumes_from_its_checkpoint_after_a_restart() {
    let Some(h) = harness().await else {
        return skip("scan_resume");
    };

    // Queue a thin scan over everything.
    let queued = post_json(
        &h.app,
        "/api/v1/prune/scans",
        json!({ "scope": { "kind": "all" }, "detectors": ["thin"] }),
    )
    .await;
    let scan_id = queued.json()["id"].as_i64().unwrap();

    // Simulate a crash mid-walk: the row is `running` with a checkpoint at a
    // mid-corpus document id and some pages already examined.
    let cursor = "https://example.com/zzz-nonexistent-cursor"; // after every example.com id
    let already_examined: i64 = 9;
    sqlx::query(
        "UPDATE ovis.prune_scan SET status = 'running', started_at = now(), \
         examined = $2, checkpoint = $3 WHERE id = $1",
    )
    .bind(scan_id)
    .bind(already_examined)
    .bind(json!({ "done": [], "phase": "documents", "cursor": cursor }))
    .execute(&h.state.db)
    .await
    .unwrap();

    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.document WHERE id > $1")
            .bind(cursor)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert!(remaining > 0, "the fixture needs ids after the cursor");

    assert!(prune_scan::run_next_scan(&h.state).await);

    let done = get(&h.app, &format!("/api/v1/prune/scans/{scan_id}")).await.json();
    assert_eq!(done["status"], "done");
    assert_eq!(
        done["examined"].as_i64().unwrap(),
        already_examined + remaining,
        "resume must continue from the checkpoint, not re-walk: {done}"
    );

    // Only documents after the cursor were examined, so only the paused.example
    // stubs (which sort after the cursor) are candidates.
    let body = get(&h.app, "/api/v1/prune/candidates?detector=thin&limit=100")
        .await
        .json();
    let flagged: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["document_id"].as_str().unwrap())
        .collect();
    assert!(!flagged.contains(&"https://example.com/active-stub"), "before the cursor");
    assert!(flagged.contains(&"https://paused.example/old-stub"), "after the cursor");

    let audit = get(&h.app, "/api/v1/prune/audit?action=scan_resumed").await.json();
    assert_eq!(audit["total"], 1, "the resume is audited");
}

#[tokio::test]
async fn starter_rules_ship_disabled_and_preview_never_mutates() {
    let Some(h) = harness().await else {
        return skip("starter_rules_preview");
    };

    let rules = get(&h.app, "/api/v1/prune/rules").await.json();
    let rules = rules.as_array().unwrap().clone();
    assert_eq!(rules.len(), 3, "the starter pack");
    for rule in &rules {
        assert_eq!(rule["enabled"], false, "starter rules ship disabled");
        assert_eq!(rule["kind"], "url_rule");
    }

    // Preview the tracking-params rule against live data.
    let tracking = rules
        .iter()
        .find(|r| r["name"] == "tracking-params")
        .unwrap();
    let reply = post_json(
        &h.app,
        &format!("/api/v1/prune/rules/{}/preview", tracking["id"].as_i64().unwrap()),
        json!({}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let preview = reply.json();
    assert_eq!(preview["matched"], 2, "{preview}");
    assert_eq!(preview["complete"], true);
    let sampled: Vec<&str> = preview["sample"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["document_id"].as_str().unwrap())
        .collect();
    assert!(
        sampled.contains(&"https://paused.example/dup?utm_source=feed"),
        "{sampled:?}"
    );

    // Preview created no candidates and no audit actions beyond rule CRUD.
    let candidates: i64 = sqlx::query_scalar("SELECT count(*) FROM ovis.prune_candidate")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(candidates, 0, "a preview never mutates");
}

// ===========================================================================
// v2 — measurement-based detection
// ===========================================================================

/// Asset URLs are junk; PDFs on this corpus are not.
///
/// The distinction matters at scale: the reference corpus holds 64k image URLs
/// whose extracted text is `name.png (W×H)`, and 88k PDFs that are real
/// content (government report mirrors, scanned technical archives). A detector
/// that treated "binary" as "junk" would delete the second group.
#[tokio::test]
async fn asset_urls_are_flagged_and_pdfs_are_left_alone() {
    let Some(h) = harness().await else {
        return skip("asset_urls_are_flagged_and_pdfs_are_left_alone");
    };

    let scan = run_scan(&h, json!({ "kind": "all" }), json!(["url_junk"])).await;
    assert_eq!(scan["status"], "done", "{scan}");
    assert!(scan["stats"]["asset_hits"].as_i64().unwrap() >= 1);

    let candidates = get(&h.app, "/api/v1/prune/candidates?detector=url_junk&limit=100")
        .await
        .json();
    let flagged: Vec<&str> = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["document_id"].as_str().unwrap())
        .collect();

    assert!(
        flagged.contains(&"https://paused.example/media/diagram.png"),
        "the image URL must be flagged: {flagged:?}"
    );
    assert!(
        !flagged.contains(&"https://paused.example/reports/annual.pdf"),
        "PDFs carry real content on this corpus and must not be flagged: {flagged:?}"
    );

    // The reason carries what it measured, not just a verdict.
    let asset = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["document_id"] == "https://paused.example/media/diagram.png")
        .unwrap();
    let reason = &asset["reasons"][0];
    assert_eq!(reason["detector"], "url_junk");
    assert_eq!(reason["code"], "asset_url");
    assert_eq!(reason["evidence"]["url_class"], "image");
}

/// Two URLs that differ only by scheme, `www.`, a trailing slash and a
/// tracking parameter are one page — even though their content hashes differ,
/// which is exactly the case `exact_duplicate` cannot see.
#[tokio::test]
async fn url_variants_are_grouped_even_when_content_hashes_differ() {
    let Some(h) = harness().await else {
        return skip("url_variants_are_grouped_even_when_content_hashes_differ");
    };

    // The canonical key is computed during the document walk, so the variant
    // phase needs a walk detector alongside it.
    let scan = run_scan(&h, json!({ "kind": "all" }), json!(["url_junk", "url_variant"])).await;
    assert_eq!(scan["status"], "done", "{scan}");
    // Two groups: the news/story pair, and — correctly — the exact-duplicate
    // fixture, whose `?utm_source=feed` copy canonicalises onto the clean URL.
    // The two duplicate detectors agreeing about that pair is the intended
    // behaviour; they reach it by different evidence.
    assert_eq!(
        scan["stats"]["url_variant_groups"], 2,
        "both canonical-URL groups in the fixture: {scan}"
    );
    assert_eq!(scan["stats"]["url_variant_hits"], 2, "one non-keeper each");

    let candidates = get(&h.app, "/api/v1/prune/candidates?limit=100").await.json();
    let variant = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["document_id"] == "http://www.paused.example/news/story/?utm_source=newsletter"
        })
        .expect("the news/story variant is flagged");

    // The tracked, www, trailing-slash copy is the one flagged; the clean URL
    // is the keeper (shortest URL wins under the default policy).
    assert_eq!(
        variant["document_id"],
        "http://www.paused.example/news/story/?utm_source=newsletter"
    );
    let reason = variant["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["code"] == "url_variant_of")
        .unwrap();
    assert_eq!(reason["evidence"]["kept"], "https://paused.example/news/story");
    assert_eq!(reason["evidence"]["group_size"], 2);
}

/// Quality gates measure every document and flag only those failing several
/// checks across distinct families.
#[tokio::test]
async fn quality_gates_flag_navigation_chrome_and_spare_real_prose() {
    let Some(h) = harness().await else {
        return skip("quality_gates_flag_navigation_chrome_and_spare_real_prose");
    };

    let scan = run_scan(&h, json!({ "kind": "all" }), json!(["quality"])).await;
    assert_eq!(scan["status"], "done", "{scan}");
    assert!(
        scan["stats"]["quality_measured"].as_i64().unwrap() > 0,
        "gates must run: {scan}"
    );
    assert!(
        scan["stats"]["profiles_written"].as_i64().unwrap() > 0,
        "every examined document gets a profile: {scan}"
    );

    let candidates = get(&h.app, "/api/v1/prune/candidates?detector=quality&limit=100")
        .await
        .json();
    let flagged: Vec<&str> = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["document_id"].as_str().unwrap())
        .collect();

    assert!(
        flagged.contains(&"https://paused.example/site/nav"),
        "a page of navigation links must be flagged: {flagged:?}"
    );
    assert!(
        !flagged.contains(&"https://paused.example/guide"),
        "an operations guide is real prose and must survive: {flagged:?}"
    );

    // Confidence stays below certainty: these are heuristics about text shape.
    let nav = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["document_id"] == "https://paused.example/site/nav")
        .unwrap();
    let confidence = nav["confidence"].as_f64().unwrap();
    assert!(
        (0.4..=0.85).contains(&confidence),
        "quality confidence must stay modest, got {confidence}"
    );
    let reason = &nav["reasons"][0];
    assert_eq!(reason["detector"], "quality");
    assert!(
        reason["evidence"]["families"].as_i64().unwrap() >= 2,
        "failures must span families: {reason}"
    );
    assert!(
        !reason["evidence"]["explanations"]
            .as_array()
            .unwrap()
            .is_empty(),
        "each failure explains its measurement and threshold"
    );
}

/// Profiles are written for every document a scan examines, including the ones
/// it decides are fine — that is what lets a policy be simulated afterwards
/// without re-scanning.
#[tokio::test]
async fn profiles_record_measurements_for_unflagged_documents_too() {
    let Some(h) = harness().await else {
        return skip("profiles_record_measurements_for_unflagged_documents_too");
    };

    run_scan(&h, json!({ "kind": "all" }), json!(["quality", "url_junk"])).await;

    let (profiled, flagged): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM ovis.doc_profile), \
                (SELECT count(*) FROM ovis.prune_candidate WHERE state = 'candidate')",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert!(
        profiled > flagged,
        "profiles ({profiled}) must cover more than the flagged set ({flagged})"
    );

    // A specific unflagged document still has its measurements on file.
    let row: (Option<i32>, Option<String>, i16) = sqlx::query_as(
        "SELECT word_count, url_class, quality_fail_count FROM ovis.doc_profile \
         WHERE document_id = 'https://paused.example/guide'",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert!(row.0.unwrap_or(0) > 0, "word count recorded");
    assert_eq!(row.1.as_deref(), Some("page"));

    // And the canonical URL is recorded for every document, so a later
    // url_variant scan needs no re-measurement.
    let canonical: Option<String> = sqlx::query_scalar(
        "SELECT canonical_url FROM ovis.doc_profile \
         WHERE document_id = 'http://www.paused.example/news/story/?utm_source=newsletter'",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(
        canonical.as_deref(),
        Some("http://paused.example/news/story"),
        "scheme, www, trailing slash and tracking parameter all fold"
    );
}

/// Verified pairs are stored with their similarity so the acting threshold can
/// move later without recomputing signatures.
#[tokio::test]
async fn near_duplicate_pairs_are_stored_for_later_rethresholding() {
    let Some(h) = harness().await else {
        return skip("near_duplicate_pairs_are_stored_for_later_rethresholding");
    };

    let scan = run_scan(&h, json!({ "kind": "all" }), json!(["near_duplicate"])).await;
    assert_eq!(scan["status"], "done", "{scan}");

    let pairs: Vec<(String, String, Option<f32>, Option<bool>)> = sqlx::query_as(
        "SELECT a, b, estimated, same_connector FROM ovis.dup_pair WHERE method = 'minhash'",
    )
    .fetch_all(&h.state.db)
    .await
    .unwrap();
    assert!(!pairs.is_empty(), "the guide/guide-copy pair must be stored");

    let (a, b, estimated, same_connector) = &pairs[0];
    assert!(a < b, "pairs are stored in a canonical order");
    assert!(
        estimated.unwrap() >= 0.8,
        "the stored similarity is the measured one"
    );
    assert_eq!(
        *same_connector,
        Some(true),
        "cross-connector pairs are held to a stricter band, so the flag is recorded"
    );

    // The profile carries each side's strongest similarity.
    let max_jaccard: Option<f32> = sqlx::query_scalar(
        "SELECT max_jaccard FROM ovis.doc_profile WHERE document_id = $1",
    )
    .bind(a)
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert!(max_jaccard.unwrap() >= 0.8);
}

// ===========================================================================
// v2 — triage, simulation, and the trash, over HTTP
// ===========================================================================

/// The funnel groups the backlog into a handful of reviewable bundles instead
/// of one very long list.
#[tokio::test]
async fn the_overview_groups_candidates_into_described_bundles() {
    let Some(h) = harness().await else {
        return skip("the_overview_groups_candidates_into_described_bundles");
    };
    run_scan(
        &h,
        json!({ "kind": "all" }),
        json!(["thin", "exact_duplicate", "url_junk"]),
    )
    .await;

    let reply = get(&h.app, "/api/v1/prune/overview").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = reply.json();
    assert!(body["candidates_open"].as_i64().unwrap() > 0);
    assert!(body["profiled"].as_i64().unwrap() > 0);
    assert_eq!(body["documents_total"].as_i64().unwrap(), 24);

    let bundles = body["bundles"].as_array().unwrap();
    assert!(!bundles.is_empty(), "the backlog must be grouped: {body}");
    for bundle in bundles {
        assert!(!bundle["title"].as_str().unwrap().is_empty());
        assert!(
            bundle["description"].as_str().unwrap().len() > 40,
            "each bundle explains itself: {bundle}"
        );
        assert!(bundle["documents"].as_i64().unwrap() > 0);
    }
    // Reclaim weight is reported, since that is what deleting actually buys.
    assert!(bundles.iter().any(|b| b["chunks"].as_i64().unwrap() > 0));

    // Trash counts ride along so the shell can show the recovery window.
    assert_eq!(body["trash"]["items"], 0);
}

/// Simulating a policy reports what it would do and changes nothing.
#[tokio::test]
async fn simulating_a_policy_reports_bands_without_creating_anything() {
    let Some(h) = harness().await else {
        return skip("simulating_a_policy_reports_bands_without_creating_anything");
    };
    // Profiles only — no candidates yet.
    run_scan(&h, json!({ "kind": "all" }), json!(["quality", "url_junk"])).await;

    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM ovis.prune_candidate")
        .fetch_one(&h.state.db)
        .await
        .unwrap();

    let reply = post_json(
        &h.app,
        "/api/v1/prune/simulate",
        json!({ "tier": "standard", "sample": 5 }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = reply.json();

    assert_eq!(body["tier"], "standard");
    assert!(body["profiled"].as_i64().unwrap() > 0, "{body}");
    let auto = body["auto"].as_i64().unwrap();
    let review = body["review"].as_i64().unwrap();
    let untouched = body["untouched"].as_i64().unwrap();
    assert_eq!(
        auto + review + untouched,
        body["profiled"].as_i64().unwrap(),
        "the three bands must partition the profiled set: {body}"
    );
    assert!(auto > 0, "stubs and duplicates should reach the auto band: {body}");

    // Boundary samples let a threshold be checked against real documents.
    assert!(
        !body["auto_sample"].as_array().unwrap().is_empty(),
        "a sample must be drawn: {body}"
    );
    assert!(
        !body["auto_sample"][0]["signals"]
            .as_array()
            .unwrap()
            .is_empty(),
        "each sampled document says which signals put it there"
    );

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM ovis.prune_candidate")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(before, after, "simulation must not create candidates");
}

/// The three presets are ordered, and the ordering is visible in the counts.
#[tokio::test]
async fn presets_are_ordered_from_conservative_to_aggressive() {
    let Some(h) = harness().await else {
        return skip("presets_are_ordered_from_conservative_to_aggressive");
    };
    run_scan(
        &h,
        json!({ "kind": "all" }),
        json!(["quality", "url_junk", "near_duplicate"]),
    )
    .await;

    let mut totals = Vec::new();
    for tier in ["conservative", "standard", "aggressive"] {
        let reply = post_json(&h.app, "/api/v1/prune/simulate", json!({ "tier": tier })).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
        let body = reply.json();
        totals.push(body["auto"].as_i64().unwrap() + body["review"].as_i64().unwrap());
    }
    assert!(
        totals[0] <= totals[1] && totals[1] <= totals[2],
        "each preset must catch at least as much as the last: {totals:?}"
    );
}

/// A policy body that cannot mean what it says is refused before it runs.
#[tokio::test]
async fn an_incoherent_policy_is_refused_with_the_reason() {
    let Some(h) = harness().await else {
        return skip("an_incoherent_policy_is_refused_with_the_reason");
    };
    let reply = post_json(
        &h.app,
        "/api/v1/prune/simulate",
        json!({ "policy": { "near_duplicate": { "auto": 0.5, "review": 0.9 } } }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
    assert!(reply.body.contains("stronger claim"), "{}", reply.body);

    let reply = post_json(&h.app, "/api/v1/prune/simulate", json!({ "tier": "nuclear" })).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.body.contains("conservative"), "{}", reply.body);
}

/// Committing a policy turns a band into candidates, and the count has to
/// match what the caller last simulated.
#[tokio::test]
async fn committing_a_policy_requires_the_simulated_count() {
    let Some(h) = harness().await else {
        return skip("committing_a_policy_requires_the_simulated_count");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["quality", "url_junk"])).await;

    let simulated = post_json(&h.app, "/api/v1/prune/simulate", json!({ "tier": "standard" }))
        .await
        .json();
    let auto = simulated["auto"].as_i64().unwrap();
    assert!(auto > 0);

    // A stale count is a 409 and nothing is created.
    let stale = post_json(
        &h.app,
        "/api/v1/prune/policies/commit",
        json!({ "tier": "standard", "band": "auto", "confirm_count": auto + 1 }),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.body);
    assert!(stale.body.contains(&auto.to_string()), "{}", stale.body);

    let reply = post_json(
        &h.app,
        "/api/v1/prune/policies/commit",
        json!({
            "tier": "standard",
            "band": "auto",
            "confirm_count": auto,
            "save_as": "house-standard"
        }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = reply.json();
    assert_eq!(body["created"], auto);
    assert_eq!(body["saved_as"], "house-standard");

    // The saved policy is now the active one.
    let policies = get(&h.app, "/api/v1/prune/policies").await.json();
    let saved = policies["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "house-standard")
        .expect("the policy was saved");
    assert_eq!(saved["active"], true);
    assert_eq!(saved["tier"], "standard");

    // Candidates carry the policy provenance.
    let candidates = get(&h.app, "/api/v1/prune/candidates?limit=5").await.json();
    let policy_candidate = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["reasons"][0]["detector"] == "policy")
        .expect("a policy-created candidate");
    assert_eq!(policy_candidate["reasons"][0]["evidence"]["band"], "auto");
    assert!(!policy_candidate["reasons"][0]["evidence"]["policy_hash"]
        .as_str()
        .unwrap()
        .is_empty());
}

/// Duplicate clusters are returned whole, keeper first — the unit of review.
#[tokio::test]
async fn clusters_return_the_whole_group_with_its_keeper() {
    let Some(h) = harness().await else {
        return skip("clusters_return_the_whole_group_with_its_keeper");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["exact_duplicate"])).await;

    let reply = get(&h.app, "/api/v1/prune/clusters?method=hash&limit=10").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = reply.json();
    let clusters = body["items"].as_array().unwrap();
    assert!(!clusters.is_empty(), "{body}");

    let cluster = &clusters[0];
    assert_eq!(cluster["method"], "hash");
    let members = cluster["members"].as_array().unwrap();
    assert!(members.len() >= 2, "a cluster has at least two members");
    assert_eq!(
        members[0]["is_keeper"], true,
        "the keeper is listed first so review starts from what survives"
    );
    assert_eq!(
        members.iter().filter(|m| m["is_keeper"] == true).count(),
        1,
        "exactly one keeper per cluster"
    );
    assert!(
        !cluster["keeper_reason"].as_str().unwrap().is_empty(),
        "the UI must be able to say why this member survives"
    );
    // Non-keepers link to their candidate row so the cluster can be actioned.
    assert!(members
        .iter()
        .filter(|m| m["is_keeper"] == false)
        .all(|m| m["candidate_id"].is_i64()));
}

/// A deployment with no model configured must see exactly today's screen. The
/// narration field is absent rather than present-and-empty, so the UI can tell
/// "not narrated" from "narrated, and the model had nothing to say".
#[tokio::test]
async fn clusters_carry_no_narration_until_one_is_generated() {
    let Some(h) = harness().await else {
        return skip("clusters_carry_no_narration_until_one_is_generated");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["exact_duplicate"])).await;

    let body = get(&h.app, "/api/v1/prune/clusters?method=hash&limit=10")
        .await
        .json();
    let clusters = body["items"].as_array().unwrap();
    assert!(!clusters.is_empty(), "{body}");
    for cluster in clusters {
        assert!(
            cluster.get("narration").is_none(),
            "an unnarrated cluster must not carry the key at all: {cluster}"
        );
        // And the mechanical description it has today is untouched.
        assert!(!cluster["keeper_reason"].as_str().unwrap().is_empty());
    }

    let overview = get(&h.app, "/api/v1/prune/overview").await.json();
    for bundle in overview["bundles"].as_array().unwrap() {
        assert!(bundle.get("narration").is_none(), "{bundle}");
        assert!(!bundle["description"].as_str().unwrap().is_empty());
    }
}

/// The review surfaces must survive the annotation table being unreadable.
///
/// A generated title is decoration; Triage and Clusters compute nothing from
/// it. But the lookup used to propagate its error, so an instance whose
/// `llm_enabled` was true while `ovis.llm_annotation` was missing — the shape
/// of a rolling upgrade, and the shape this suite's own harness was in —
/// answered 500 on both screens instead of showing the mechanical
/// descriptions.
#[tokio::test]
async fn a_missing_annotation_table_costs_the_titles_not_the_screen() {
    let Some(h) = harness().await else {
        return skip("a_missing_annotation_table_costs_the_titles_not_the_screen");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["exact_duplicate", "thin"])).await;
    assert!(h.state.llm_enabled, "the harness must have the LLM path live");

    sqlx::query("DROP TABLE ovis.llm_annotation")
        .execute(&h.state.db)
        .await
        .expect("the annotation table is droppable");

    let overview = get(&h.app, "/api/v1/prune/overview").await;
    assert_eq!(overview.status, StatusCode::OK, "{}", overview.body);
    let bundles = overview.json()["bundles"].as_array().unwrap().clone();
    assert!(!bundles.is_empty(), "the funnel still groups the backlog");
    for bundle in &bundles {
        assert!(bundle.get("narration").is_none(), "{bundle}");
        assert!(!bundle["description"].as_str().unwrap().is_empty());
    }

    let clusters = get(&h.app, "/api/v1/prune/clusters?method=hash&limit=10").await;
    assert_eq!(clusters.status, StatusCode::OK, "{}", clusters.body);
    assert!(!clusters.json()["items"].as_array().unwrap().is_empty());

    ovis_core::db::annotation::ensure_tables(&h.state.db).await;
}

/// A duplicate mirrored across two connectors is reviewed, not bulk-staged.
///
/// `cross_connector_review_only` shipped as a field every preset set to true
/// and no predicate read, so the documented behaviour — FineWeb's finding that
/// global dedup over-prunes, because a document mirrored across sources is
/// usually popular rather than redundant — did nothing at all. This drives the
/// setting from both sides against real rows.
#[tokio::test]
async fn a_duplicate_mirrored_across_connectors_is_held_back_from_the_bulk_band() {
    let Some(h) = harness().await else {
        return skip("a_duplicate_mirrored_across_connectors_is_held_back_from_the_bulk_band");
    };

    // Same content on two different connectors. The shorter URL keeps.
    for (id, connector) in [
        ("https://paused.example/mirror", 2),
        ("https://example.com/mirrored/copy", 1),
    ] {
        sqlx::query(
            "INSERT INTO public.document \
                 (id, boost, hidden, semantic_id, link, last_modified, chunk_count, \
                  doc_metadata, from_ingestion_api, content_hash) \
             VALUES ($1, 0, false, $1, $1, now() - interval '20 days', 3, '{}'::jsonb, \
                     false, 'prune-mirror-hash')",
        )
        .bind(id)
        .execute(&h.state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO public.document_by_connector_credential_pair \
                 (id, connector_id, credential_id, has_been_indexed) VALUES ($1, $2, 1, true)",
        )
        .bind(id)
        .bind(connector)
        .execute(&h.state.db)
        .await
        .unwrap();
    }

    run_scan(&h, json!({ "kind": "all" }), json!(["exact_duplicate"])).await;

    let cross: bool = sqlx::query_scalar(
        "SELECT cross_connector FROM ovis.doc_dup_group \
         WHERE document_id = $1 AND method = 'hash'",
    )
    .bind("https://example.com/mirrored/copy")
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert!(cross, "the scan records the connector spread");

    // The same-connector fixture group must stay unmarked, or the guard would
    // hold back every duplicate rather than the mirrored ones.
    let same: bool = sqlx::query_scalar(
        "SELECT cross_connector FROM ovis.doc_dup_group \
         WHERE document_id = $1 AND method = 'hash'",
    )
    .bind("https://paused.example/dup/print/view")
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert!(!same, "a single-connector group is not mirrored");

    // The scan also opened a candidate for every non-keeper, and policy never
    // re-flags a document that already has one. Clear them so the bands are
    // computed from the measurements, which is what is under test here.
    sqlx::query("DELETE FROM ovis.prune_candidate")
        .execute(&h.state.db)
        .await
        .unwrap();

    use ovis_core::db::profile::{documents_in_band, Band, Policy};
    let mut policy = Policy::standard();
    assert!(policy.cross_connector_review_only, "the preset asks for it");

    let auto = documents_in_band(&h.state.db, &policy, Band::Auto, None, 500)
        .await
        .unwrap();
    let review = documents_in_band(&h.state.db, &policy, Band::Review, None, 500)
        .await
        .unwrap();
    let mirrored = "https://example.com/mirrored/copy".to_string();
    assert!(
        !auto.contains(&mirrored),
        "a mirrored copy must not be staged in bulk: {auto:?}"
    );
    assert!(
        review.contains(&mirrored),
        "it must still be surfaced for review, not dropped: {review:?}"
    );
    assert!(
        auto.contains(&"https://paused.example/dup/print/view".to_string()),
        "a same-connector duplicate is unaffected: {auto:?}"
    );

    // Turning the rule off is what moves it, and nothing else does.
    policy.cross_connector_review_only = false;
    let auto = documents_in_band(&h.state.db, &policy, Band::Auto, None, 500)
        .await
        .unwrap();
    assert!(
        auto.contains(&mirrored),
        "with the rule off the mirrored copy joins the bulk band: {auto:?}"
    );
}

/// The two duplicate detectors do not evict each other.
///
/// Membership used to be one column on `doc_profile`, so a document grouped by
/// both content hash and canonical URL kept only whichever phase ran last —
/// and the URL phase runs second. A three-document hash cluster came back from
/// the API with one member and no keeper, and the `exact_duplicate` policy
/// signal stopped matching copies that were still byte-identical. Found by
/// running both detectors in one scan and reading the clusters screen.
#[tokio::test]
async fn a_document_can_belong_to_a_hash_group_and_a_url_group_at_once() {
    let Some(h) = harness().await else {
        return skip("a_document_can_belong_to_a_hash_group_and_a_url_group_at_once");
    };
    // The fixture's `?utm_source=feed` copy is both: identical content to
    // `/dup`, and the same canonical URL once tracking parameters are folded.
    run_scan(
        &h,
        json!({ "kind": "all" }),
        json!(["exact_duplicate", "url_junk", "url_variant"]),
    )
    .await;

    let both: Vec<(String, i32, bool)> = sqlx::query_as(
        "SELECT method, group_size, is_keeper FROM ovis.doc_dup_group \
         WHERE document_id = $1 ORDER BY method",
    )
    .bind("https://paused.example/dup?utm_source=feed")
    .fetch_all(&h.state.db)
    .await
    .unwrap();
    assert_eq!(
        both.len(),
        2,
        "the document is in both a hash group and a URL group: {both:?}"
    );
    assert_eq!(both[0].0, "hash");
    assert_eq!(both[0].1, 3, "the hash group keeps all three members");
    assert_eq!(both[1].0, "url");

    // And the clusters screen returns the whole hash group, keeper included.
    let reply = get(&h.app, "/api/v1/prune/clusters?method=hash&limit=10").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = reply.json();
    let cluster = &body["items"].as_array().unwrap()[0];
    let members = cluster["members"].as_array().unwrap();
    assert_eq!(
        members.len(),
        3,
        "every member of the hash group survives the URL phase: {cluster}"
    );
    assert_eq!(
        members.iter().filter(|m| m["is_keeper"] == true).count(),
        1,
        "and the keeper is still one of them: {cluster}"
    );
    assert_eq!(cluster["size"], 3);
}

/// URL clusters are keyed by their canonical URL, one group per page.
///
/// The key is everything after the first colon of `url:<canonical>`; taking
/// the second colon-separated field instead yielded the literal `http` for
/// every group in the corpus, so every URL variant on the deployment collapsed
/// into one unbounded cluster and the pagination cursor could not advance.
#[tokio::test]
async fn url_clusters_are_keyed_by_canonical_url_not_by_its_scheme() {
    let Some(h) = harness().await else {
        return skip("url_clusters_are_keyed_by_canonical_url_not_by_its_scheme");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["url_junk", "url_variant"])).await;

    let reply = get(&h.app, "/api/v1/prune/clusters?method=url&limit=10").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let body = reply.json();
    let clusters = body["items"].as_array().unwrap();

    // The fixture has two canonical-URL groups; they must arrive as two.
    assert_eq!(clusters.len(), 2, "one cluster per canonical URL: {body}");
    for cluster in clusters {
        assert_eq!(cluster["method"], "url");
        let key = cluster["key"].as_str().unwrap();
        assert!(
            key.contains("://"),
            "the key is the canonical URL, not its scheme: {key}"
        );
        let members = cluster["members"].as_array().unwrap();
        assert_eq!(
            members.len() as i64,
            cluster["size"].as_i64().unwrap(),
            "every member of the group is present: {cluster}"
        );
        assert_eq!(
            members.iter().filter(|m| m["is_keeper"] == true).count(),
            1,
            "exactly one keeper per cluster: {cluster}"
        );
    }

    // And the cursor advances rather than returning the same page forever.
    let first = clusters[0]["key"].as_str().unwrap();
    let next = get(
        &h.app,
        &format!(
            "/api/v1/prune/clusters?method=url&limit=10&after={}",
            percent_encoding::utf8_percent_encode(first, percent_encoding::NON_ALPHANUMERIC)
        ),
    )
    .await
    .json();
    for cluster in next["items"].as_array().unwrap() {
        assert!(
            cluster["key"].as_str().unwrap() > first,
            "paging past {first} must not return it again: {cluster}"
        );
    }
}

/// Pressing the button with nothing assigned has to say what to do about it,
/// not fail with a lookup error from three layers down.
#[tokio::test]
async fn narrating_without_an_assigned_model_says_what_to_do() {
    let Some(h) = harness().await else {
        return skip("narrating_without_an_assigned_model_says_what_to_do");
    };

    let reply = post_json(
        &h.app,
        "/api/v1/prune/narrate",
        json!({ "subject_kind": "cluster" }),
    )
    .await;
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert!(reply.body.contains("narrate role"), "{}", reply.body);
    assert!(reply.body.contains("Models page"), "{}", reply.body);
}

/// The sampling plan states, in words, what accepting the sample would mean.
#[tokio::test]
async fn the_sampling_plan_states_its_statistical_claim() {
    let Some(h) = harness().await else {
        return skip("the_sampling_plan_states_its_statistical_claim");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["thin", "exact_duplicate"])).await;

    let body = get(&h.app, "/api/v1/prune/sample?detector=duplicate&n=3")
        .await
        .json();
    assert!(body["population"].as_i64().unwrap() > 0, "{body}");
    assert!(body["sample_size"].as_i64().unwrap() > 0);
    assert_eq!(body["confidence"], 0.95);

    let statement = body["statement"].as_str().unwrap();
    assert!(statement.contains("95% confidence"), "{statement}");
    assert!(
        statement.contains("tighten this group's threshold"),
        "the plan must say what to do when the sample fails: {statement}"
    );
    assert_eq!(
        body["documents"].as_array().unwrap().len(),
        body["sample_size"].as_i64().unwrap() as usize
    );
}

/// The grace deadline is measured by the database's clock, not the server's.
///
/// The reaper's due filter is `stage_expires_at <= now()`, evaluated in
/// Postgres. Writing the deadline from the application clock made the grace
/// period wrong by however much the two disagreed — and on a container running
/// 23 ms ahead of Postgres, `OVIS_PRUNE_GRACE_DAYS=0` (documented and
/// supported) produced staged documents that were never due, silently. One
/// clock, or the window is a guess.
#[tokio::test]
async fn the_grace_deadline_is_written_by_the_same_clock_that_judges_it() {
    let Some(h) = harness_with(false, |cfg| cfg.prune_grace_days = 0).await else {
        return skip("the_grace_deadline_is_written_by_the_same_clock_that_judges_it");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;
    let candidates = get(&h.app, "/api/v1/prune/candidates?limit=50").await.json();
    let id = candidates["items"][0]["id"].as_i64().unwrap();

    let staged = post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "ids": [id], "confirm_count": 1 }),
    )
    .await;
    assert_eq!(staged.status, StatusCode::OK, "{}", staged.body);

    // Postgres' own verdict, which is the one the reaper asks for.
    let due: bool = sqlx::query_scalar(
        "SELECT stage_expires_at <= now() FROM ovis.prune_candidate WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert!(
        due,
        "with a zero grace period the document must be due the instant it is staged"
    );

    // And the response reports the deadline that was actually written.
    let reported = staged.json()["stage_expires_at"].as_str().unwrap().to_string();
    let stored: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT stage_expires_at FROM ovis.prune_candidate WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&reported).unwrap(),
        stored,
        "the API must report the stored deadline, not one it predicted"
    );
}

/// The whole point, end to end over HTTP: stage, let the reaper delete, find
/// the document gone from Onyx but present in the trash, and put it back.
#[tokio::test]
async fn a_deleted_document_lands_in_the_trash_and_can_be_restored_over_http() {
    let Some(h) = harness_with(false, |cfg| cfg.prune_grace_days = 0).await else {
        return skip("a_deleted_document_lands_in_the_trash_and_can_be_restored_over_http");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;

    let candidates = get(&h.app, "/api/v1/prune/candidates?limit=50").await.json();
    let target = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["document_id"] == "https://paused.example/old-stub")
        .expect("the paused stub is a candidate");
    let candidate_id = target["id"].as_i64().unwrap();

    let staged = post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "ids": [candidate_id], "confirm_count": 1 }),
    )
    .await;
    assert_eq!(staged.status, StatusCode::OK, "{}", staged.body);

    let report = prune_reaper::run_cycle(&h.state).await.expect("reaper cycle");
    assert_eq!(report.deleted, 1, "the reaper deletes the due document");

    // Gone from Onyx.
    assert!(
        doc_hidden(&h.state, "https://paused.example/old-stub")
            .await
            .is_none(),
        "the document row must be gone"
    );

    // Present in the trash, with its recovery window.
    let trash = get(&h.app, "/api/v1/prune/trash").await.json();
    assert_eq!(trash["total"], 1, "{trash}");
    let item = &trash["items"][0];
    assert_eq!(item["document_id"], "https://paused.example/old-stub");
    assert_eq!(item["hold"], false);
    assert_eq!(item["reappeared"], false);
    assert!(item["snapshot_bytes"].as_i64().unwrap() > 0);
    assert!(
        item["expires_at"].as_str().is_some(),
        "the retention deadline is shown, not implied"
    );

    // The content is readable without restoring first.
    let encoded = urlencoding_encode("https://paused.example/old-stub");
    let detail = get(&h.app, &format!("/api/v1/prune/trash/{encoded}")).await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.body);
    let detail = detail.json();
    assert_eq!(detail["document"]["semantic_id"], "Old Stub");

    // The delete outcome records that it was trashed, not merely deleted.
    let audit = get(&h.app, "/api/v1/prune/audit?action=deleted").await.json();
    assert_eq!(audit["items"][0]["detail"]["trashed"], true, "{audit}");

    // Restore it.
    let restored = post_json(
        &h.app,
        "/api/v1/prune/trash/restore",
        json!({ "document_ids": ["https://paused.example/old-stub"], "confirm_count": 1 }),
    )
    .await;
    assert_eq!(restored.status, StatusCode::OK, "{}", restored.body);
    let body = restored.json();
    assert_eq!(body["changed"], 1);
    assert_eq!(body["action"], "restored");

    // Back in Onyx, with its original flags.
    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/old-stub").await,
        Some(false),
        "the document is back, un-hidden as it was before staging"
    );
    let trash = get(&h.app, "/api/v1/prune/trash").await.json();
    assert_eq!(trash["total"], 0, "a restored snapshot leaves the trash");
}

/// Purging is the one irreversible verb and demands the typed count at any
/// size; a held snapshot is never swept up by it.
#[tokio::test]
async fn purging_demands_a_typed_count_and_respects_holds() {
    let Some(h) = harness_with(false, |cfg| cfg.prune_grace_days = 0).await else {
        return skip("purging_demands_a_typed_count_and_respects_holds");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;

    // Stage and delete every stub so there is something in the trash.
    let candidates = get(&h.app, "/api/v1/prune/candidates?limit=50").await.json();
    let ids: Vec<i64> = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    let staged = post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "ids": ids, "confirm_count": ids.len() }),
    )
    .await;
    assert_eq!(staged.status, StatusCode::OK, "{}", staged.body);
    prune_reaper::run_cycle(&h.state).await.expect("reaper cycle");

    let trash = get(&h.app, "/api/v1/prune/trash").await.json();
    let total = trash["total"].as_i64().unwrap();
    assert!(total >= 2, "several documents should be in the trash: {trash}");
    let first = trash["items"][0]["document_id"].as_str().unwrap().to_string();

    // Without a typed count, purge refuses and says why.
    let refused = post_json(
        &h.app,
        "/api/v1/prune/trash/purge",
        json!({ "document_ids": [first.clone()], "confirm_count": 1 }),
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
    assert!(refused.body.contains("cannot be undone"), "{}", refused.body);

    // Hold it, then a correct typed count still refuses to destroy it.
    let held = post_json(
        &h.app,
        "/api/v1/prune/trash/hold",
        json!({ "document_ids": [first.clone()], "hold": true }),
    )
    .await;
    assert_eq!(held.status, StatusCode::OK, "{}", held.body);

    let attempt = post_json(
        &h.app,
        "/api/v1/prune/trash/purge",
        json!({ "document_ids": [first.clone()], "confirm_count": 1, "typed_count": 1 }),
    )
    .await;
    assert_eq!(attempt.status, StatusCode::OK, "{}", attempt.body);
    let body = attempt.json();
    assert_eq!(body["changed"], 0, "a held snapshot survives: {body}");
    assert_eq!(body["failed"][0]["code"], "ON_HOLD");

    let trash = get(&h.app, "/api/v1/prune/trash").await.json();
    assert_eq!(trash["total"], total, "nothing was destroyed");
}

/// Percent-encode a document id for use in a path segment.
fn urlencoding_encode(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => {
                let mut buf = [0u8; 4];
                other
                    .encode_utf8(&mut buf)
                    .bytes()
                    .map(|b| format!("%{b:02X}"))
                    .collect()
            }
        })
        .collect()
}

/// A document that was already hidden before pruning restores to hidden; one
/// that was visible restores to visible. The snapshot records the document's
/// own state, not the staging flag pruning set on the way to deleting it.
#[tokio::test]
async fn restore_returns_the_hidden_flag_the_document_had_before_pruning() {
    let Some(h) = harness_with(false, |cfg| cfg.prune_grace_days = 0).await else {
        return skip("restore_returns_the_hidden_flag_the_document_had_before_pruning");
    };
    run_scan(&h, json!({ "kind": "all" }), json!(["thin"])).await;

    let candidates = get(&h.app, "/api/v1/prune/candidates?limit=50").await.json();
    let hidden_before = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["document_id"] == "https://paused.example/already-hidden-stub")
        .expect("the already-hidden stub is a candidate");
    let visible_before = candidates["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["document_id"] == "https://paused.example/old-stub")
        .expect("the visible stub is a candidate");

    let ids = json!([hidden_before["id"], visible_before["id"]]);
    let staged = post_json(
        &h.app,
        "/api/v1/prune/candidates/stage",
        json!({ "ids": ids, "confirm_count": 2 }),
    )
    .await;
    assert_eq!(staged.status, StatusCode::OK, "{}", staged.body);
    prune_reaper::run_cycle(&h.state).await.expect("reaper cycle");

    let restored = post_json(
        &h.app,
        "/api/v1/prune/trash/restore",
        json!({
            "document_ids": [
                "https://paused.example/already-hidden-stub",
                "https://paused.example/old-stub"
            ],
            "confirm_count": 2
        }),
    )
    .await;
    assert_eq!(restored.status, StatusCode::OK, "{}", restored.body);

    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/already-hidden-stub").await,
        Some(true),
        "a document hidden before pruning comes back hidden"
    );
    assert_eq!(
        doc_hidden(&h.state, "https://paused.example/old-stub").await,
        Some(false),
        "a visible document comes back visible, not stuck behind the staging flag"
    );
}
