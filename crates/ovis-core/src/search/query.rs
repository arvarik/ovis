//! OpenSearch request bodies, built as pure functions.
//!
//! Keeping the JSON out of the HTTP client makes it testable without a server,
//! and these bodies are where the old implementation's cost lived: a chunk fetch
//! with no `_source` filter downloaded full 768-float embedding arrays for every
//! chunk of every row on every list request.

use serde_json::{json, Value};

use crate::api_types::SearchMode;

/// Fields that must never come back in a bulk read: three float arrays of 768
/// values each, per chunk.
pub const VECTOR_FIELDS: [&str; 3] = ["embeddings", "content_vector", "title_vector"];

/// Long text fields that a metadata-only chunk listing does not need.
const BULKY_TEXT_FIELDS: [&str; 4] = ["content", "chunk_context", "doc_summary", "blurb"];

/// `_source` fields a search hit needs. Deliberately narrow — a search result is
/// a pointer to a document, not the document.
const SEARCH_SOURCE_FIELDS: [&str; 7] = [
    "document_id",
    "chunk_index",
    "title",
    "semantic_identifier",
    "blurb",
    "source_type",
    "last_updated",
];

/// One chunk page for the detail view.
///
/// `document_id` is a `keyword` field, so this is a plain `term` — no
/// `.keyword` suffix and no `should` pair between the two spellings, which is
/// what the old client did because it did not know the mapping.
pub fn chunks_body(document_id: &str, after: Option<i64>, size: i64, include_content: bool) -> Value {
    let mut excludes: Vec<&str> = VECTOR_FIELDS.to_vec();
    if !include_content {
        excludes.extend_from_slice(&BULKY_TEXT_FIELDS);
    }

    let mut body = json!({
        "query": { "term": { "document_id": document_id } },
        "_source": { "excludes": excludes },
        "sort": [{ "chunk_index": "asc" }],
        "size": size,
        // Without this, `total` saturates at 10000 and a long document silently
        // reports the wrong chunk total.
        "track_total_hits": true
    });

    // search_after replaces the old hard `size: 500` cap, which truncated any
    // document with more than 500 chunks and reported the truncated number.
    if let Some(after) = after {
        body["search_after"] = json!([after]);
    }
    body
}

/// One chunk's stored vector, by deterministic `_id`.
pub fn chunk_id(document_id: &str, chunk_index: i64) -> String {
    format!("{document_id}__{chunk_index}")
}

/// Chunk counts for up to 500 documents in one round-trip. Used to detect
/// Postgres↔index drift (orphaned chunks), never on the list path.
pub fn terms_agg_body(document_ids: &[String]) -> Value {
    json!({
        "size": 0,
        "query": { "terms": { "document_id": document_ids } },
        "aggs": {
            "by_doc": {
                "terms": { "field": "document_id", "size": document_ids.len().max(1) }
            }
        }
    })
}

pub fn delete_by_query_body(document_ids: &[String]) -> Value {
    if document_ids.len() == 1 {
        json!({ "query": { "term": { "document_id": document_ids[0] } } })
    } else {
        json!({ "query": { "terms": { "document_id": document_ids } } })
    }
}

/// Propagate a title edit into the chunks of one document.
pub fn update_title_body(document_id: &str, title: &str) -> Value {
    json!({
        "script": {
            "source": "ctx._source.title = params.title; ctx._source.semantic_identifier = params.title;",
            "lang": "painless",
            "params": { "title": title }
        },
        "query": { "term": { "document_id": document_id } }
    })
}

/// Propagate `hidden`/`boost` into the chunks of one document. Only used when no
/// Onyx API key is configured — with a key, Onyx applies these and syncs its own
/// index.
pub fn update_flags_body(document_id: &str, hidden: Option<bool>, boost: Option<i32>) -> Option<Value> {
    let mut script = String::new();
    let mut params = serde_json::Map::new();
    if let Some(hidden) = hidden {
        script.push_str("ctx._source.hidden = params.hidden; ");
        params.insert("hidden".into(), json!(hidden));
    }
    if let Some(boost) = boost {
        script.push_str("ctx._source.global_boost = params.boost; ");
        params.insert("boost".into(), json!(boost));
    }
    if script.is_empty() {
        return None;
    }
    Some(json!({
        "script": { "source": script.trim_end(), "lang": "painless", "params": params },
        "query": { "term": { "document_id": document_id } }
    }))
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchFilters {
    /// Matched against the index's `source_type`, which is lower-case (`web`),
    /// unlike Postgres's `connector.source` (`WEB`).
    pub source: Option<String>,
    /// Include chunks Onyx has hidden from search. Off by default.
    pub include_hidden: bool,
}

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: String,
    pub mode: SearchMode,
    pub filters: SearchFilters,
    pub limit: i64,
    pub offset: i64,
    /// Query embedding, present only when the mode needs one *and* the embedder
    /// answered.
    pub vector: Option<Vec<f32>>,
    /// The `knn_vector` field to search. `None` means this index cannot serve
    /// kNN, so semantic/hybrid degrade to BM25.
    pub knn_field: Option<String>,
}

fn filter_clauses(filters: &SearchFilters) -> Vec<Value> {
    let mut clauses = Vec::new();
    if let Some(source) = &filters.source {
        clauses.push(json!({ "term": { "source_type": source.to_lowercase() } }));
    }
    if !filters.include_hidden {
        clauses.push(json!({ "term": { "hidden": false } }));
    }
    clauses
}

fn text_clause(query: &str, boost: Option<f64>) -> Value {
    let mut clause = json!({
        "multi_match": {
            "query": query,
            "type": "best_fields",
            "fields": ["title^3", "semantic_identifier^3", "blurb^2", "content"]
        }
    });
    if let Some(boost) = boost {
        clause["multi_match"]["boost"] = json!(boost);
    }
    clause
}

/// Build the search body for whichever mode is actually servable.
///
/// `collapse` on `document_id` makes a result list one row per *document* (its
/// best-matching chunk) rather than a wall of near-duplicate chunk hits.
pub fn search_body(req: &SearchRequest) -> Value {
    let filters = filter_clauses(&req.filters);
    let knn_clause = req.knn_field.as_ref().zip(req.vector.as_ref()).map(|(field, vector)| {
        json!({ "knn": { field: { "vector": vector, "k": 50, "boost": 0.6 } } })
    });

    // Only a semantic request that *got* a usable kNN clause ends up with no text
    // clause. Everything else — including a semantic or hybrid request that had
    // to fall back — searches text and therefore has terms worth highlighting.
    let vectors_only = req.mode == SearchMode::Semantic && knn_clause.is_some();

    let query = match (req.mode, knn_clause) {
        // Semantic: vectors only.
        (SearchMode::Semantic, Some(knn)) => json!({
            "bool": { "should": [knn], "minimum_should_match": 1, "filter": filters }
        }),
        // Hybrid: BM25 and vectors, weighted.
        (SearchMode::Hybrid, Some(knn)) => json!({
            "bool": {
                "should": [text_clause(&req.query, Some(0.4)), knn],
                "minimum_should_match": 1,
                "filter": filters
            }
        }),
        // Keyword, or a semantic/hybrid request that had to fall back.
        _ => json!({
            "bool": { "must": [text_clause(&req.query, None)], "filter": filters }
        }),
    };

    let mut body = json!({
        "query": query,
        "_source": SEARCH_SOURCE_FIELDS,
        "collapse": { "field": "document_id" },
        "size": req.limit,
        "from": req.offset,
        "track_total_hits": 10_000
    });

    if !vectors_only {
        body["highlight"] = json!({
            "fields": { "content": { "fragment_size": 160, "number_of_fragments": 2 } }
        });
    }
    body
}

/// Chunk count for one `source_type`. `source_type` is a `text` field in this
/// mapping, so it cannot be aggregated — one cheap count per source instead.
pub fn source_count_body(source: &str) -> Value {
    json!({
        "size": 0,
        "track_total_hits": true,
        "query": { "term": { "source_type": source.to_lowercase() } }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_fetch_always_excludes_vectors() {
        let body = chunks_body("https://example.com/a", None, 100, true);
        let excludes = body["_source"]["excludes"].as_array().unwrap();
        for field in VECTOR_FIELDS {
            assert!(
                excludes.iter().any(|v| v == field),
                "{field} must be excluded; downloading it is the old N+1's real cost"
            );
        }
        // Content is what the caller asked for.
        assert!(!excludes.iter().any(|v| v == "content"));
    }

    #[test]
    fn meta_only_chunk_fetch_drops_the_bulky_text_too() {
        let body = chunks_body("https://example.com/a", None, 100, false);
        let excludes = body["_source"]["excludes"].as_array().unwrap();
        for field in ["content", "blurb", "chunk_context", "doc_summary"] {
            assert!(excludes.iter().any(|v| v == field), "{field} not excluded");
        }
    }

    #[test]
    fn chunk_fetch_uses_a_plain_term_on_the_keyword_field() {
        let body = chunks_body("https://example.com/a", None, 100, true);
        assert_eq!(body["query"]["term"]["document_id"], "https://example.com/a");
        let text = body.to_string();
        assert!(
            !text.contains("document_id.keyword"),
            "document_id is already a keyword; the .keyword dance is wrong"
        );
        assert!(!text.contains("should"), "no should-pair needed");
    }

    #[test]
    fn chunk_fetch_tracks_the_real_total_and_pages_with_search_after() {
        let first = chunks_body("d", None, 100, true);
        assert_eq!(first["track_total_hits"], true);
        assert!(first.get("search_after").is_none());
        assert_eq!(first["sort"][0]["chunk_index"], "asc");

        let next = chunks_body("d", Some(99), 100, true);
        assert_eq!(next["search_after"], json!([99]));
    }

    #[test]
    fn chunk_id_is_the_deterministic_opensearch_id() {
        assert_eq!(
            chunk_id("https://example.com/a", 5),
            "https://example.com/a__5"
        );
    }

    #[test]
    fn single_delete_uses_term_and_batch_uses_terms() {
        let one = delete_by_query_body(&["a".to_string()]);
        assert_eq!(one["query"]["term"]["document_id"], "a");

        let many = delete_by_query_body(&["a".to_string(), "b".to_string()]);
        assert_eq!(many["query"]["terms"]["document_id"], json!(["a", "b"]));
    }

    #[test]
    fn keyword_search_filters_hidden_and_highlights() {
        let req = SearchRequest {
            query: "tax reform".into(),
            mode: SearchMode::Keyword,
            filters: SearchFilters::default(),
            limit: 20,
            offset: 0,
            vector: None,
            knn_field: None,
        };
        let body = search_body(&req);
        assert_eq!(body["collapse"]["field"], "document_id");
        assert_eq!(body["query"]["bool"]["filter"][0]["term"]["hidden"], false);
        assert!(body["highlight"]["fields"]["content"].is_object());
        assert!(body["query"]["bool"]["must"][0]["multi_match"].is_object());
        assert!(body["query"]["bool"]["should"].is_null());
    }

    #[test]
    fn source_filter_is_lower_cased_for_the_index() {
        // Postgres stores WEB; the index stores web.
        let req = SearchRequest {
            query: "q".into(),
            mode: SearchMode::Keyword,
            filters: SearchFilters {
                source: Some("WEB".into()),
                include_hidden: false,
            },
            limit: 10,
            offset: 0,
            vector: None,
            knn_field: None,
        };
        let body = search_body(&req);
        assert_eq!(body["query"]["bool"]["filter"][0]["term"]["source_type"], "web");
    }

    #[test]
    fn include_hidden_drops_the_hidden_filter() {
        let req = SearchRequest {
            query: "q".into(),
            mode: SearchMode::Keyword,
            filters: SearchFilters {
                source: None,
                include_hidden: true,
            },
            limit: 10,
            offset: 0,
            vector: None,
            knn_field: None,
        };
        let body = search_body(&req);
        assert_eq!(body["query"]["bool"]["filter"], json!([]));
    }

    #[test]
    fn hybrid_combines_weighted_bm25_with_knn() {
        let req = SearchRequest {
            query: "tax reform".into(),
            mode: SearchMode::Hybrid,
            filters: SearchFilters::default(),
            limit: 20,
            offset: 0,
            vector: Some(vec![0.1, 0.2, 0.3]),
            knn_field: Some("embeddings.full_embedding".into()),
        };
        let body = search_body(&req);
        let should = body["query"]["bool"]["should"].as_array().unwrap();
        assert_eq!(should.len(), 2);
        assert_eq!(should[0]["multi_match"]["boost"], 0.4);
        let knn = &should[1]["knn"]["embeddings.full_embedding"];
        assert_eq!(knn["boost"], 0.6);
        assert_eq!(knn["k"], 50);
        // f32 widened to f64 in JSON, so compare with tolerance.
        let sent: Vec<f64> = knn["vector"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert_eq!(sent.len(), 3);
        for (got, want) in sent.iter().zip([0.1_f64, 0.2, 0.3]) {
            assert!((got - want).abs() < 1e-6, "{got} != {want}");
        }
    }

    #[test]
    fn semantic_drops_the_text_clause_and_the_highlight() {
        let req = SearchRequest {
            query: "tax reform".into(),
            mode: SearchMode::Semantic,
            filters: SearchFilters::default(),
            limit: 20,
            offset: 0,
            vector: Some(vec![0.1]),
            knn_field: Some("embeddings.full_embedding".into()),
        };
        let body = search_body(&req);
        let should = body["query"]["bool"]["should"].as_array().unwrap();
        assert_eq!(should.len(), 1);
        assert!(should[0]["knn"].is_object());
        assert!(
            body.get("highlight").is_none(),
            "a pure vector query has no terms to highlight"
        );
    }

    #[test]
    fn semantic_without_a_usable_knn_field_falls_back_to_bm25() {
        // Both the "embedder is down" case (vector: None) and the "this index
        // has no populated knn field" case (knn_field: None) must degrade to a
        // query that actually returns results, not to an empty vector query.
        for (vector, knn_field) in [
            (None, Some("embeddings.full_embedding".to_string())),
            (Some(vec![0.1]), None),
            (None, None),
        ] {
            let req = SearchRequest {
                query: "tax reform".into(),
                mode: SearchMode::Semantic,
                filters: SearchFilters::default(),
                limit: 20,
                offset: 0,
                vector,
                knn_field,
            };
            let body = search_body(&req);
            assert!(
                body["query"]["bool"]["must"][0]["multi_match"].is_object(),
                "fallback must be a real BM25 query: {body}"
            );
            assert!(body["highlight"].is_object());
        }
    }

    #[test]
    fn flag_update_body_is_none_when_nothing_changed() {
        assert!(update_flags_body("d", None, None).is_none());

        let body = update_flags_body("d", Some(true), None).unwrap();
        let src = body["script"]["source"].as_str().unwrap();
        assert!(src.contains("hidden"));
        assert!(!src.contains("global_boost"));

        let body = update_flags_body("d", Some(false), Some(5)).unwrap();
        assert_eq!(body["script"]["params"]["boost"], 5);
        assert_eq!(body["script"]["params"]["hidden"], false);
    }

    #[test]
    fn title_update_writes_both_title_fields() {
        let body = update_title_body("d", "New Title");
        let src = body["script"]["source"].as_str().unwrap();
        assert!(src.contains("ctx._source.title"));
        assert!(src.contains("ctx._source.semantic_identifier"));
        assert_eq!(body["script"]["params"]["title"], "New Title");
        assert_eq!(body["query"]["term"]["document_id"], "d");
    }
}
