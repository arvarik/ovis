//! The OpenSearch HTTP client.
//!
//! Hand-rolled over `reqwest` rather than the `opensearch` crate: six query
//! shapes do not justify the dependency, and the crate lags OpenSearch 3.x. What
//! it does have that the old client did not: timeouts on every call, a bounded
//! connection pool, and an index name that comes from `search_settings` instead
//! of the `danswer_chunk*` wildcard.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;

use crate::api_types::ChunkItem;
use crate::error::{CoreError, CoreResult};

use super::query;

/// OpenSearch caps a `terms` query at 65,536 clauses by default, but 500 keeps
/// request bodies and aggregation buckets sane.
pub const MAX_TERMS_PER_REQUEST: usize = 500;

#[derive(Debug, Clone)]
pub struct OsClient {
    client: reqwest::Client,
    base_url: String,
    credentials: Option<(String, String)>,
}

/// What this particular index can actually do.
///
/// Probed rather than assumed. On the gamma deployment the mapping declares
/// `embeddings.full_embedding` as a 768-dim `knn_vector` (hnsw/lucene/cosine) but
/// **zero documents populate it** — Onyx writes its vectors to `content_vector`,
/// which is typed as a plain `float` array and cannot serve kNN or a
/// `script_score` cosine. A kNN query against that index returns zero hits in
/// 1 ms, which would look like "semantic search found nothing" rather than
/// "semantic search is not available here".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexCapabilities {
    /// A `knn_vector` field with documents in it, if any. `None` ⇒ this index
    /// cannot serve semantic or hybrid search and both degrade to BM25.
    pub knn_field: Option<String>,
    /// A field in `_source` holding a per-chunk float array, used to answer
    /// "show me this chunk's real vector". Falls back to `content_vector` when
    /// the knn field is derived-source (and so unreadable) or empty.
    pub source_vector_field: Option<String>,
}

impl IndexCapabilities {
    pub fn knn_ready(&self) -> bool {
        self.knn_field.is_some()
    }
}

impl OsClient {
    pub fn new(base_url: &str, username: Option<&str>, password: Option<&str>) -> CoreResult<Self> {
        let client = reqwest::Client::builder()
            // A hung OpenSearch node used to stall a request forever while
            // holding its Postgres pool slot. Now it fails in seconds.
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(8)
            .build()
            .map_err(|e| CoreError::search(format!("cannot build OpenSearch client: {e}")))?;

        let credentials = match (username, password) {
            (Some(u), Some(p)) if !u.is_empty() => Some((u.to_string(), p.to_string())),
            _ => None,
        };

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            credentials,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.credentials {
            Some((u, p)) => req.basic_auth(u, Some(p)),
            None => req,
        }
    }

    async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> CoreResult<Value> {
        let response = self
            .auth(req)
            .send()
            .await
            .map_err(|e| CoreError::search(format!("{what}: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            // The body goes to the caller's log, never to an HTTP client.
            let body = response.text().await.unwrap_or_default();
            return Err(CoreError::search(format!(
                "{what}: HTTP {status}: {}",
                truncate(&body, 800)
            )));
        }

        response
            .json()
            .await
            .map_err(|e| CoreError::search(format!("{what}: malformed response body: {e}")))
    }

    async fn post(&self, path: &str, body: &Value, what: &str) -> CoreResult<Value> {
        let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        self.send(self.client.post(&url).json(body), what).await
    }

    async fn get(&self, path: &str, what: &str) -> CoreResult<Value> {
        let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        self.send(self.client.get(&url), what).await
    }

    /// Cluster liveness plus round-trip latency.
    pub async fn ping(&self) -> CoreResult<Duration> {
        let started = Instant::now();
        self.get("/", "opensearch ping").await?;
        Ok(started.elapsed())
    }

    // -----------------------------------------------------------------------
    // Chunks
    // -----------------------------------------------------------------------

    /// One page of a document's chunks, ordered by `chunk_index`.
    ///
    /// Returns the items, the true total chunk count, and the `search_after`
    /// value for the next page.
    pub async fn document_chunks(
        &self,
        index: &str,
        document_id: &str,
        after: Option<i64>,
        size: i64,
        include_content: bool,
    ) -> CoreResult<(Vec<ChunkItem>, i64, Option<i64>)> {
        let body = query::chunks_body(document_id, after, size, include_content);
        let response = self
            .post(
                &format!("{}/_search", encode_index(index)),
                &body,
                "fetch document chunks",
            )
            .await?;

        let total = response["hits"]["total"]["value"].as_i64().unwrap_or(0);
        let hits = response["hits"]["hits"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let items: Vec<ChunkItem> = hits.iter().map(parse_chunk).collect();

        // Only advertise a next page when this one was full.
        let next_after = (items.len() as i64 == size)
            .then(|| items.last().map(|c| c.chunk_index))
            .flatten();

        Ok((items, total, next_after))
    }

    /// One chunk's stored vector.
    ///
    /// Tries the deterministic `_id` (`{document_id}__{chunk_index}`) first, which
    /// is a single-shard GET. That scheme holds for most documents but not all —
    /// some ids in this index differ from their Postgres `document.id` by a
    /// trailing slash — so a miss falls back to a `term` query on the
    /// `document_id` keyword plus `chunk_index`, which is authoritative.
    ///
    /// `field` comes from [`IndexCapabilities::source_vector_field`]. Returns
    /// `Ok(None)` when the chunk genuinely has no vector — the honest answer, as
    /// opposed to the UI's previous habit of fabricating one.
    pub async fn chunk_vector(
        &self,
        index: &str,
        document_id: &str,
        chunk_index: i64,
        field: &str,
    ) -> CoreResult<Option<Vec<f32>>> {
        let id = query::chunk_id(document_id, chunk_index);
        let path = format!(
            "{}/_doc/{}?_source_includes={}&ignore=404",
            encode_index(index),
            encode_path_segment(&id),
            encode_path_segment(field)
        );
        // A 404 here is "not under that id", not an error, so do not let it
        // surface as an upstream failure.
        if let Ok(response) = self.get(&path, "fetch chunk vector").await {
            if response["found"].as_bool().unwrap_or(false) {
                return Ok(extract_vector(&response["_source"], field));
            }
        }

        let body = serde_json::json!({
            "size": 1,
            "query": { "bool": { "filter": [
                { "term": { "document_id": document_id } },
                { "term": { "chunk_index": chunk_index } }
            ]}},
            "_source": { "includes": [field] }
        });
        let response = self
            .post(
                &format!("{}/_search", encode_index(index)),
                &body,
                "fetch chunk vector",
            )
            .await?;

        let Some(hit) = response["hits"]["hits"].as_array().and_then(|h| h.first()) else {
            return Ok(None);
        };
        Ok(extract_vector(&hit["_source"], field))
    }

    /// Chunk counts for up to [`MAX_TERMS_PER_REQUEST`] documents in one call.
    pub async fn chunk_counts(
        &self,
        index: &str,
        document_ids: &[String],
    ) -> CoreResult<HashMap<String, i64>> {
        if document_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut counts = HashMap::new();
        for batch in document_ids.chunks(MAX_TERMS_PER_REQUEST) {
            let body = query::terms_agg_body(batch);
            let response = self
                .post(
                    &format!("{}/_search", encode_index(index)),
                    &body,
                    "count chunks by document",
                )
                .await?;
            if let Some(buckets) = response["aggregations"]["by_doc"]["buckets"].as_array() {
                for bucket in buckets {
                    if let (Some(key), Some(count)) =
                        (bucket["key"].as_str(), bucket["doc_count"].as_i64())
                    {
                        counts.insert(key.to_string(), count);
                    }
                }
            }
        }
        Ok(counts)
    }

    /// Reconstruct a document's full text from its chunks, in order.
    pub async fn document_text(&self, index: &str, document_id: &str) -> CoreResult<String> {
        let mut parts: Vec<String> = Vec::new();
        let mut after: Option<i64> = None;
        loop {
            let (items, _total, next) = self
                .document_chunks(index, document_id, after, 200, true)
                .await?;
            if items.is_empty() {
                break;
            }
            parts.extend(items.into_iter().filter_map(|c| c.content));
            match next {
                Some(n) => after = Some(n),
                None => break,
            }
        }
        Ok(parts.join("\n\n"))
    }

    // -----------------------------------------------------------------------
    // Mutations
    // -----------------------------------------------------------------------

    /// Remove every chunk of one document.
    ///
    /// `refresh=true` so a client that re-reads immediately after its own delete
    /// sees the result; `conflicts=proceed` so a concurrent Onyx re-index does
    /// not abort the delete partway.
    pub async fn delete_document_chunks(&self, index: &str, document_id: &str) -> CoreResult<u64> {
        self.delete_chunks_for(index, std::slice::from_ref(&document_id.to_string()))
            .await
    }

    /// Batch variant: one `_delete_by_query` per [`MAX_TERMS_PER_REQUEST`] ids.
    pub async fn delete_chunks_for(&self, index: &str, document_ids: &[String]) -> CoreResult<u64> {
        if document_ids.is_empty() {
            return Ok(0);
        }
        let mut deleted = 0u64;
        for batch in document_ids.chunks(MAX_TERMS_PER_REQUEST) {
            let body = query::delete_by_query_body(batch);
            let response = self
                .post(
                    &format!(
                        "{}/_delete_by_query?refresh=true&conflicts=proceed",
                        encode_index(index)
                    ),
                    &body,
                    "delete document chunks",
                )
                .await?;
            deleted += response["deleted"].as_u64().unwrap_or(0);
        }
        Ok(deleted)
    }

    /// Chunk text for many documents in **one** round trip.
    ///
    /// The scan's content pass is latency-bound, not throughput-bound: one
    /// query per document against a LAN OpenSearch measured 137 documents/s,
    /// which is three and a half hours for a 1.7 M-document corpus. `_msearch`
    /// sends the same per-document queries in a single request, so the cost
    /// becomes one round trip per page instead of one per document.
    ///
    /// Deliberately *not* a single `terms` query: per-document result caps and
    /// `chunk_index` ordering are what keep a 5,000-chunk document from
    /// crowding out everything else on the page, and a shared query cannot
    /// express them.
    ///
    /// Returns, per input id in order, the chunks and the true total.
    pub async fn document_chunks_batch(
        &self,
        index: &str,
        document_ids: &[String],
        size: i64,
        include_content: bool,
    ) -> CoreResult<Vec<(Vec<ChunkItem>, i64)>> {
        if document_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(document_ids.len());
        // Bounded so one request never carries an unreasonable number of
        // sub-queries; OpenSearch's default `max_concurrent_searches` applies
        // per request, not per sub-query.
        for batch in document_ids.chunks(200) {
            let mut ndjson = String::new();
            for document_id in batch {
                ndjson.push_str("{}\n");
                let body = query::chunks_body(document_id, None, size, include_content);
                ndjson.push_str(&body.to_string());
                ndjson.push('\n');
            }
            let url = format!("{}/{}/_msearch", self.base_url, encode_index(index));
            let response = self
                .send(
                    self.client
                        .post(&url)
                        .header("Content-Type", "application/x-ndjson")
                        .body(ndjson),
                    "batch fetch document chunks",
                )
                .await?;

            let responses = response["responses"].as_array().cloned().unwrap_or_default();
            if responses.len() != batch.len() {
                return Err(CoreError::search(format!(
                    "batch fetch document chunks: asked for {} documents, got {} responses",
                    batch.len(),
                    responses.len()
                )));
            }
            for sub in responses {
                // A per-sub-query error must not be read as "no chunks" —
                // that would silently thin the candidate set, which is exactly
                // what the consecutive-error guard exists to prevent.
                if let Some(error) = sub.get("error") {
                    return Err(CoreError::search(format!(
                        "batch fetch document chunks: {}",
                        truncate(&error.to_string(), 300)
                    )));
                }
                let total = sub["hits"]["total"]["value"].as_i64().unwrap_or(0);
                let items: Vec<ChunkItem> = sub["hits"]["hits"]
                    .as_array()
                    .map(|hits| hits.iter().map(parse_chunk).collect())
                    .unwrap_or_default();
                out.push((items, total));
            }
        }
        Ok(out)
    }

    /// Every chunk of one document as its verbatim `_id` and `_source`,
    /// **including** the embedding vectors that [`Self::document_chunks`]
    /// deliberately excludes.
    ///
    /// This is the trash snapshot's read path, and the reason a restored
    /// document is searchable again the moment it comes back: the vectors
    /// return with it, so nothing has to be re-embedded. Ordinary chunk
    /// browsing must keep excluding vectors — 768 floats per chunk is a
    /// hundredfold size increase for a UI that never displays them.
    pub async fn document_chunks_raw(
        &self,
        index: &str,
        document_id: &str,
        after: Option<i64>,
        size: i64,
    ) -> CoreResult<(Vec<(String, Value)>, i64, Option<i64>)> {
        let mut body = serde_json::json!({
            "query": { "term": { "document_id": document_id } },
            "sort": [{ "chunk_index": "asc" }],
            "size": size,
            "track_total_hits": true
        });
        if let Some(after) = after {
            body["search_after"] = serde_json::json!([after]);
        }
        let response = self
            .post(
                &format!("{}/_search", encode_index(index)),
                &body,
                "fetch raw document chunks",
            )
            .await?;

        let total = response["hits"]["total"]["value"].as_i64().unwrap_or(0);
        let hits = response["hits"]["hits"].as_array().cloned().unwrap_or_default();
        let items: Vec<(String, Value)> = hits
            .iter()
            .map(|hit| {
                (
                    hit["_id"].as_str().unwrap_or_default().to_string(),
                    hit["_source"].clone(),
                )
            })
            .collect();

        let next_after = (items.len() as i64 == size)
            .then(|| {
                items
                    .last()
                    .and_then(|(_, src)| src["chunk_index"].as_i64())
            })
            .flatten();
        Ok((items, total, next_after))
    }

    /// Re-index chunks verbatim under their original `_id`s — the trash
    /// restore path.
    ///
    /// Uses `_bulk` with `index` (not `create`) actions so a restore is
    /// idempotent: replaying it over chunks that already came back overwrites
    /// them rather than erroring out halfway through.
    pub async fn bulk_index_chunks(&self, index: &str, chunks: &[(String, Value)]) -> CoreResult<u64> {
        if chunks.is_empty() {
            return Ok(0);
        }
        let mut indexed = 0u64;
        for batch in chunks.chunks(200) {
            let mut ndjson = String::new();
            for (id, source) in batch {
                let action = serde_json::json!({ "index": { "_id": id } });
                ndjson.push_str(&action.to_string());
                ndjson.push('\n');
                ndjson.push_str(&source.to_string());
                ndjson.push('\n');
            }
            let url = format!(
                "{}/{}/_bulk?refresh=true",
                self.base_url,
                encode_index(index)
            );
            let response = self
                .send(
                    self.client
                        .post(&url)
                        .header("Content-Type", "application/x-ndjson")
                        .body(ndjson),
                    "bulk index chunks",
                )
                .await?;
            // `_bulk` answers 200 even when individual items failed; the
            // per-item errors are the only honest signal.
            if response["errors"].as_bool().unwrap_or(false) {
                let first = response["items"]
                    .as_array()
                    .and_then(|items| {
                        items
                            .iter()
                            .find_map(|item| item["index"]["error"]["reason"].as_str())
                    })
                    .unwrap_or("unknown");
                return Err(CoreError::search(format!(
                    "bulk index chunks: some items failed, first reason: {}",
                    truncate(first, 300)
                )));
            }
            indexed += response["items"].as_array().map(|i| i.len() as u64).unwrap_or(0);
        }
        Ok(indexed)
    }

    pub async fn update_document_title(
        &self,
        index: &str,
        document_id: &str,
        title: &str,
    ) -> CoreResult<u64> {
        let body = query::update_title_body(document_id, title);
        let response = self
            .post(
                &format!(
                    "{}/_update_by_query?conflicts=proceed&refresh=true",
                    encode_index(index)
                ),
                &body,
                "update chunk titles",
            )
            .await?;
        Ok(response["updated"].as_u64().unwrap_or(0))
    }

    pub async fn update_document_flags(
        &self,
        index: &str,
        document_id: &str,
        hidden: Option<bool>,
        boost: Option<i32>,
    ) -> CoreResult<u64> {
        let Some(body) = query::update_flags_body(document_id, hidden, boost) else {
            return Ok(0);
        };
        let response = self
            .post(
                &format!(
                    "{}/_update_by_query?conflicts=proceed&refresh=true",
                    encode_index(index)
                ),
                &body,
                "update chunk flags",
            )
            .await?;
        Ok(response["updated"].as_u64().unwrap_or(0))
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    pub async fn search(
        &self,
        index: &str,
        request: &super::SearchRequest,
    ) -> CoreResult<RawSearchResults> {
        let body = query::search_body(request);
        let response = self
            .post(
                &format!("{}/_search", encode_index(index)),
                &body,
                "search chunks",
            )
            .await?;

        let took_ms = response["took"].as_u64().unwrap_or(0);
        let total = response["hits"]["total"]["value"].as_i64().unwrap_or(0);
        let total_exact = response["hits"]["total"]["relation"]
            .as_str()
            .map(|r| r == "eq")
            .unwrap_or(false);

        let hits = response["hits"]["hits"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(parse_search_hit)
            .collect();

        Ok(RawSearchResults {
            hits,
            total,
            total_exact,
            took_ms,
        })
    }

    /// Chunk count for one `source_type` value.
    pub async fn count_by_source(&self, index: &str, source: &str) -> CoreResult<i64> {
        let body = query::source_count_body(source);
        let response = self
            .post(
                &format!("{}/_search", encode_index(index)),
                &body,
                "count chunks by source",
            )
            .await?;
        Ok(response["hits"]["total"]["value"].as_i64().unwrap_or(0))
    }

    // -----------------------------------------------------------------------
    // Cluster / index health
    // -----------------------------------------------------------------------

    pub async fn cat_index(&self, index: &str) -> CoreResult<Option<Value>> {
        let path = format!("_cat/indices/{}?format=json&bytes=b", encode_index(index));
        let response = self.get(&path, "read index stats").await?;
        Ok(response.as_array().and_then(|a| a.first().cloned()))
    }

    pub async fn cat_allocation(&self) -> CoreResult<Option<Value>> {
        let response = self
            .get(
                "_cat/allocation?format=json&bytes=b",
                "read disk allocation",
            )
            .await?;
        Ok(response.as_array().and_then(|a| a.first().cloned()))
    }

    pub async fn cluster_health(&self) -> CoreResult<Value> {
        self.get("_cluster/health?format=json", "read cluster health")
            .await
    }

    /// `true` when the index carries a read-only-allow-delete block, which is
    /// what OpenSearch sets when the disk flood-stage watermark trips. This
    /// deployment has hit it before at 400 GB, so it is surfaced rather than
    /// discovered during an incident.
    pub async fn index_read_only(&self, index: &str) -> CoreResult<bool> {
        let response = self
            .get(
                &format!("{}/_settings", encode_index(index)),
                "read index settings",
            )
            .await?;
        let blocked = response
            .as_object()
            .and_then(|indices| indices.values().next())
            .and_then(|entry| entry["settings"]["index"]["blocks"].as_object().cloned())
            .map(|blocks| {
                ["read_only_allow_delete", "read_only", "write"]
                    .iter()
                    .any(|key| is_truthy(blocks.get(*key)))
            })
            .unwrap_or(false);
        Ok(blocked)
    }

    /// Work out what this index can serve. Called at startup and on every
    /// `RuntimeMeta` refresh, so a re-embed switchover is picked up
    /// automatically.
    pub async fn probe_capabilities(&self, index: &str) -> CoreResult<IndexCapabilities> {
        let mapping = self
            .get(
                &format!("{}/_mapping", encode_index(index)),
                "read index mapping",
            )
            .await?;

        let properties = mapping
            .as_object()
            .and_then(|indices| indices.values().next())
            .map(|entry| &entry["mappings"]["properties"])
            .cloned()
            .unwrap_or(Value::Null);

        let mut knn_fields = Vec::new();
        let mut float_fields = Vec::new();
        collect_vector_fields(&properties, "", &mut knn_fields, &mut float_fields);

        // A declared knn_vector field with no documents is a trap, not a
        // capability: querying it returns zero hits and looks like "no matches".
        let mut knn_field = None;
        for candidate in &knn_fields {
            if self.field_has_documents(index, candidate).await? {
                knn_field = Some(candidate.clone());
                break;
            }
        }
        if knn_field.is_none() && !knn_fields.is_empty() {
            // Debug, not warn: this probe re-runs on every RuntimeMeta refresh, and
            // a warning every 60 seconds about an unchanged condition is noise that
            // buries the lines that matter. The caller warns once at startup and
            // again only when the capability changes.
            tracing::debug!(
                declared = ?knn_fields,
                index = %index,
                "index declares knn_vector field(s) but none contain documents"
            );
        }

        // For reading one chunk's vector, prefer the knn field, but fall back to
        // a plain float array — which is where Onyx actually keeps them here.
        let mut source_vector_field = None;
        for candidate in knn_fields.iter().chain(float_fields.iter()) {
            if self.field_has_documents(index, candidate).await? {
                source_vector_field = Some(candidate.clone());
                break;
            }
        }

        Ok(IndexCapabilities {
            knn_field,
            source_vector_field,
        })
    }

    async fn field_has_documents(&self, index: &str, field: &str) -> CoreResult<bool> {
        let body = serde_json::json!({
            "size": 0,
            "terminate_after": 1,
            "query": { "exists": { "field": field } }
        });
        let response = self
            .post(
                &format!("{}/_search", encode_index(index)),
                &body,
                "probe vector field",
            )
            .await?;
        Ok(response["hits"]["total"]["value"].as_i64().unwrap_or(0) > 0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawSearchHit {
    pub document_id: String,
    pub chunk_index: Option<i64>,
    pub score: f64,
    pub snippet: Option<String>,
    pub semantic_identifier: Option<String>,
    pub source_type: Option<String>,
    pub last_updated: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawSearchResults {
    pub hits: Vec<RawSearchHit>,
    pub total: i64,
    pub total_exact: bool,
    pub took_ms: u64,
}

// ---------------------------------------------------------------------------
// Parsing
//
// The index has drifted across Onyx versions (`chunk_id` → `chunk_index`,
// `embeddings` → `content_vector`), so field lookups tolerate both spellings
// rather than assuming one.
// ---------------------------------------------------------------------------

fn parse_chunk(hit: &Value) -> ChunkItem {
    let source = &hit["_source"];
    let chunk_index = source["chunk_index"]
        .as_i64()
        .or_else(|| source["chunk_id"].as_i64())
        .unwrap_or(0);

    let content = source["content"].as_str().map(|s| s.to_string());
    let token_estimate = content
        .as_deref()
        .map(|c| c.split_whitespace().count() as i64);

    ChunkItem {
        chunk_index,
        content,
        blurb: source["blurb"].as_str().map(|s| s.to_string()),
        title: source["title"].as_str().map(|s| s.to_string()),
        semantic_identifier: source["semantic_identifier"]
            .as_str()
            .map(|s| s.to_string()),
        source_type: source["source_type"].as_str().map(|s| s.to_string()),
        token_estimate,
        source_links: parse_source_links(&source["source_links"]),
        last_updated: parse_epoch(&source["last_updated"]),
        hidden: source["hidden"].as_bool(),
        metadata_list: source["metadata_list"].as_array().map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        }),
    }
}

fn parse_search_hit(hit: &Value) -> Option<RawSearchHit> {
    let source = &hit["_source"];
    let document_id = source["document_id"].as_str()?.to_string();

    // Prefer a highlighted fragment; fall back to the blurb so a semantic hit
    // still shows something.
    let snippet = hit["highlight"]["content"]
        .as_array()
        .and_then(|frags| {
            let joined: Vec<&str> = frags.iter().filter_map(|f| f.as_str()).collect();
            (!joined.is_empty()).then(|| joined.join(" … "))
        })
        .or_else(|| source["blurb"].as_str().map(|s| s.to_string()));

    Some(RawSearchHit {
        document_id,
        chunk_index: source["chunk_index"]
            .as_i64()
            .or_else(|| source["chunk_id"].as_i64()),
        score: hit["_score"].as_f64().unwrap_or(0.0),
        snippet,
        semantic_identifier: source["semantic_identifier"]
            .as_str()
            .or_else(|| source["title"].as_str())
            .map(|s| s.to_string()),
        source_type: source["source_type"].as_str().map(|s| s.to_string()),
        last_updated: parse_epoch(&source["last_updated"]),
    })
}

/// `source_links` is stored as a JSON *string* in this mapping, not an object.
fn parse_source_links(value: &Value) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::String(s) => serde_json::from_str(s).ok().or_else(|| Some(json_str(s))),
        other => Some(other.clone()),
    }
}

fn json_str(s: &str) -> Value {
    Value::String(s.to_string())
}

/// `last_updated` is an epoch integer. Onyx writes seconds; tolerate
/// milliseconds so a future change does not land us in 1970.
fn parse_epoch(value: &Value) -> Option<DateTime<Utc>> {
    let raw = value.as_i64()?;
    let (secs, nanos) = if raw.abs() > 100_000_000_000 {
        (raw / 1000, ((raw % 1000) * 1_000_000) as u32)
    } else {
        (raw, 0)
    };
    Utc.timestamp_opt(secs, nanos).single()
}

fn extract_vector(source: &Value, field: &str) -> Option<Vec<f32>> {
    let mut node = source;
    for segment in field.split('.') {
        node = node.get(segment)?;
    }
    let values = node.as_array()?;
    let vector: Vec<f32> = values
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect();
    (!vector.is_empty()).then_some(vector)
}

/// Walk a mapping's `properties`, collecting dotted paths of `knn_vector` fields
/// and of plain `float` fields (which is how Onyx stores vectors on this index).
fn collect_vector_fields(
    properties: &Value,
    prefix: &str,
    knn: &mut Vec<String>,
    floats: &mut Vec<String>,
) {
    let Some(fields) = properties.as_object() else {
        return;
    };
    for (name, spec) in fields {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if let Some(nested) = spec.get("properties") {
            collect_vector_fields(nested, &path, knn, floats);
            continue;
        }
        match spec["type"].as_str() {
            Some("knn_vector") => knn.push(path),
            Some("float") => floats.push(path),
            _ => {}
        }
    }
}

fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Index names come from `search_settings`, but encoding them costs nothing and
/// keeps a surprising value from turning into path traversal.
fn encode_index(index: &str) -> String {
    utf8_percent_encode(index, NON_ALPHANUMERIC)
        .to_string()
        .replace("%2D", "-")
        .replace("%5F", "_")
        .replace("%2E", ".")
}

/// Document ids are URLs; every reserved character must be escaped for a path
/// segment.
fn encode_path_segment(segment: &str) -> String {
    utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn document_ids_are_fully_escaped_in_paths() {
        let encoded = encode_path_segment("https://example.com/a?b=1&c=2__5");
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains(':'));
        assert!(!encoded.contains('?'));
        assert!(!encoded.contains('&'));
        assert!(encoded.contains("%2F"));
    }

    #[test]
    fn index_names_keep_their_readable_characters() {
        assert_eq!(
            encode_index("danswer_chunk_snowflake_arctic_embed_m"),
            "danswer_chunk_snowflake_arctic_embed_m"
        );
        // ...but a path separator would still be neutralised.
        assert!(!encode_index("a/../b").contains('/'));
    }

    #[test]
    fn chunk_parsing_reads_the_real_index_shape() {
        // Verbatim from the gamma index.
        let hit = json!({
            "_id": "https://example.com/a__5",
            "_source": {
                "chunk_index": 5,
                "content": "one two three four",
                "blurb": "a blurb",
                "title": "A Title",
                "semantic_identifier": "A Title",
                "source_type": "web",
                "hidden": false,
                "source_links": "{\"0\": \"https://example.com/a\"}",
                "metadata_list": ["Author===Someone", "Creator===Word"]
            }
        });
        let chunk = parse_chunk(&hit);
        assert_eq!(chunk.chunk_index, 5);
        assert_eq!(chunk.token_estimate, Some(4));
        assert_eq!(chunk.source_type.as_deref(), Some("web"));
        // source_links arrives as a JSON string and is parsed into real JSON.
        assert_eq!(chunk.source_links.unwrap()["0"], "https://example.com/a");
        assert_eq!(chunk.metadata_list.unwrap().len(), 2);
        assert_eq!(chunk.hidden, Some(false));
    }

    #[test]
    fn chunk_parsing_tolerates_the_older_chunk_id_spelling() {
        let hit = json!({ "_source": { "chunk_id": 3, "content": "x" } });
        assert_eq!(parse_chunk(&hit).chunk_index, 3);
    }

    #[test]
    fn token_estimate_is_absent_when_content_was_not_requested() {
        let hit = json!({ "_source": { "chunk_index": 0 } });
        let chunk = parse_chunk(&hit);
        assert_eq!(chunk.token_estimate, None);
        assert_eq!(chunk.content, None);
    }

    #[test]
    fn epoch_seconds_and_millis_both_parse() {
        // Real value observed on gamma.
        let secs = parse_epoch(&json!(1775083560i64)).unwrap();
        assert_eq!(secs.timestamp(), 1775083560);
        let millis = parse_epoch(&json!(1775083560000i64)).unwrap();
        assert_eq!(millis.timestamp(), 1775083560);
        assert!(parse_epoch(&json!(null)).is_none());
        assert!(parse_epoch(&json!("nope")).is_none());
    }

    #[test]
    fn search_hit_prefers_highlight_then_blurb() {
        let with_highlight = json!({
            "_score": 12.5,
            "_source": { "document_id": "d", "chunk_index": 2, "blurb": "fallback" },
            "highlight": { "content": ["a <em>match</em>", "another <em>match</em>"] }
        });
        let hit = parse_search_hit(&with_highlight).unwrap();
        assert_eq!(
            hit.snippet.as_deref(),
            Some("a <em>match</em> … another <em>match</em>")
        );
        assert_eq!(hit.score, 12.5);

        let without = json!({
            "_score": 1.0,
            "_source": { "document_id": "d", "blurb": "fallback" }
        });
        assert_eq!(
            parse_search_hit(&without).unwrap().snippet.as_deref(),
            Some("fallback")
        );
    }

    #[test]
    fn search_hit_without_a_document_id_is_dropped_not_faked() {
        assert!(parse_search_hit(&json!({ "_source": { "blurb": "x" } })).is_none());
    }

    #[test]
    fn vector_extraction_walks_dotted_paths() {
        let source =
            json!({ "embeddings": { "full_embedding": [0.1, 0.2] }, "content_vector": [1.0] });
        assert_eq!(
            extract_vector(&source, "embeddings.full_embedding"),
            Some(vec![0.1, 0.2])
        );
        assert_eq!(extract_vector(&source, "content_vector"), Some(vec![1.0]));
        assert_eq!(extract_vector(&source, "title_vector"), None);
        // An empty array is not a vector.
        assert_eq!(extract_vector(&json!({ "v": [] }), "v"), None);
    }

    #[test]
    fn capability_probe_finds_both_field_kinds_in_the_real_mapping() {
        // The gamma mapping: a declared knn_vector plus the float arrays Onyx
        // actually populates.
        let properties = json!({
            "embeddings": { "properties": {
                "full_embedding": { "type": "knn_vector", "dimension": 768 }
            }},
            "content_vector": { "type": "float" },
            "title_vector": { "type": "float" },
            "content": { "type": "text" },
            "document_id": { "type": "keyword" }
        });
        let mut knn = Vec::new();
        let mut floats = Vec::new();
        collect_vector_fields(&properties, "", &mut knn, &mut floats);
        assert_eq!(knn, vec!["embeddings.full_embedding"]);
        assert!(floats.contains(&"content_vector".to_string()));
        assert!(floats.contains(&"title_vector".to_string()));
        assert!(!floats.contains(&"content".to_string()));
    }

    #[test]
    fn knn_readiness_requires_a_field() {
        assert!(!IndexCapabilities::default().knn_ready());
        assert!(IndexCapabilities {
            knn_field: Some("embeddings.full_embedding".into()),
            source_vector_field: None,
        }
        .knn_ready());
    }

    #[test]
    fn read_only_block_detection_accepts_string_and_bool_forms() {
        // OpenSearch returns settings values as strings.
        assert!(is_truthy(Some(&json!("true"))));
        assert!(is_truthy(Some(&json!(true))));
        assert!(!is_truthy(Some(&json!("false"))));
        assert!(!is_truthy(None));
    }

    #[test]
    fn error_bodies_are_truncated_before_they_reach_a_log_line() {
        let long = "x".repeat(5000);
        let out = truncate(&long, 800);
        assert!(out.len() <= 801 + 2);
        assert!(out.ends_with('…'));
        assert_eq!(truncate("short", 800), "short");
    }
}
