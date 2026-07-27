//! The OVIS HTTP API client — the CLI's *only* data path.
//!
//! Redesign pillar P1: the backend is the single data plane. Nothing here opens
//! a database connection or talks to OpenSearch, and there is no credential in
//! this crate beyond an optional bearer token the user supplies.
//!
//! Responses deserialise into `ovis_core::api_types`, the same structs the
//! server serialises, so a wire-shape change breaks the build rather than
//! surfacing as a null field three layers up.

use std::time::Duration;

use ovis_core::api_types::*;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{ApiErrorEnvelope, CliError, CliResult};

/// Everything but unreserved characters, so a document id survives as exactly
/// one path segment. `/`, `:` and `?` in particular must not pass through — the
/// API contract is that ids are percent-encoded (`backend/05_AS_BUILT.md` §2.3).
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'?')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b':')
    .add(b'%')
    .add(b'[')
    .add(b']')
    .add(b'@')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b';')
    .add(b'=');

pub fn encode_id(id: &str) -> String {
    utf8_percent_encode(id, PATH_SEGMENT).to_string()
}

#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
    verbose: bool,
}

impl ApiClient {
    pub fn new(base: &str, token: Option<String>, verbose: bool) -> CliResult<Self> {
        let http = reqwest::Client::builder()
            // One client per process, so keep-alive actually keeps a connection
            // alive across the calls a single command makes.
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("ovis-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| CliError::Other(anyhow::anyhow!("cannot build the HTTP client: {e}")))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token,
            verbose,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.base, path)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self.http.request(method, self.url(path));
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        req
    }

    // -----------------------------------------------------------------------
    // Transport
    // -----------------------------------------------------------------------

    async fn send(&self, req: reqwest::RequestBuilder) -> CliResult<reqwest::Response> {
        let built = req
            .build()
            .map_err(|e| CliError::Other(anyhow::anyhow!("cannot build the request: {e}")))?;
        let method = built.method().clone();
        let url = built.url().clone();
        if self.verbose {
            eprintln!("debug: {method} {url}");
        }
        let started = std::time::Instant::now();

        let response = self
            .http
            .execute(built)
            .await
            .map_err(|e| self.transport_error(e))?;

        if self.verbose {
            eprintln!(
                "debug: <- {} in {:.0} ms",
                response.status(),
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        Ok(response)
    }

    /// A connection failure is exit 12 with a message naming the URL; a timeout
    /// or a body failure is a plain error. Distinguishing them is the difference
    /// between "start the server" and "something is wrong with the server".
    fn transport_error(&self, err: reqwest::Error) -> CliError {
        let detail = if err.is_connect() {
            // reqwest wraps hyper wraps std::io; the innermost message ("Connection
            // refused") is the only part worth showing.
            root_cause(&err)
        } else if err.is_timeout() {
            "the request timed out".to_string()
        } else {
            root_cause(&err)
        };

        if err.is_connect() || err.is_timeout() {
            CliError::Unreachable {
                url: self.base.clone(),
                detail,
            }
        } else {
            CliError::Other(anyhow::anyhow!("request failed: {detail}"))
        }
    }

    async fn decode<T: DeserializeOwned>(&self, response: reqwest::Response) -> CliResult<T> {
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| CliError::Other(anyhow::anyhow!("reading the response body: {e}")))?;

        if !status.is_success() {
            return Err(error_from_body(status.as_u16(), &body));
        }
        serde_json::from_slice(&body).map_err(|e| {
            let excerpt: String = String::from_utf8_lossy(&body).chars().take(200).collect();
            CliError::Other(anyhow::anyhow!(
                "the server sent a response this build does not understand: {e}. Body began: \
                 {excerpt}"
            ))
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> CliResult<T> {
        let response = self.send(self.request(reqwest::Method::GET, path)).await?;
        self.decode(response).await
    }

    pub async fn get_text(&self, path: &str) -> CliResult<String> {
        let response = self.send(self.request(reqwest::Method::GET, path)).await?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| CliError::Other(anyhow::anyhow!("reading the response body: {e}")))?;
        if !status.is_success() {
            return Err(error_from_body(status.as_u16(), &body));
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CliResult<T> {
        let response = self
            .send(self.request(reqwest::Method::POST, path).json(body))
            .await?;
        self.decode(response).await
    }

    /// Batch delete answers `207 Multi-Status` on partial failure, which is a
    /// success at the transport level and a partial failure at the CLI level.
    /// The caller needs both the status and the body.
    pub async fn post_raw<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CliResult<(u16, T)> {
        let response = self
            .send(self.request(reqwest::Method::POST, path).json(body))
            .await?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| CliError::Other(anyhow::anyhow!("reading the response body: {e}")))?;
        if !(200..300).contains(&status) {
            return Err(error_from_body(status, &bytes));
        }
        let decoded = serde_json::from_slice(&bytes).map_err(|e| {
            CliError::Other(anyhow::anyhow!("cannot decode the response body: {e}"))
        })?;
        Ok((status, decoded))
    }

    pub async fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CliResult<T> {
        let response = self
            .send(self.request(reqwest::Method::PATCH, path).json(body))
            .await?;
        self.decode(response).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> CliResult<T> {
        let response = self
            .send(self.request(reqwest::Method::DELETE, path))
            .await?;
        self.decode(response).await
    }

    pub async fn delete_with<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CliResult<T> {
        let response = self
            .send(self.request(reqwest::Method::DELETE, path).json(body))
            .await?;
        self.decode(response).await
    }

    // -----------------------------------------------------------------------
    // Typed endpoints
    // -----------------------------------------------------------------------

    pub async fn pages(&self, query: &str) -> CliResult<ListResponse<PageListItem>> {
        self.get(&format!("/pages{}", qs(query))).await
    }

    pub async fn page_detail(&self, id: &str) -> CliResult<PageDetail> {
        self.get(&format!("/pages/{}", encode_id(id))).await
    }

    pub async fn page_text(&self, id: &str) -> CliResult<String> {
        self.get_text(&format!("/pages/{}/text", encode_id(id)))
            .await
    }

    pub async fn page_chunks(&self, id: &str, query: &str) -> CliResult<ChunksResponse> {
        self.get(&format!("/pages/{}/chunks{}", encode_id(id), qs(query)))
            .await
    }

    pub async fn page_patch(&self, id: &str, patch: &PagePatch) -> CliResult<PatchResponse> {
        self.patch(&format!("/pages/{}", encode_id(id)), patch)
            .await
    }

    pub async fn page_delete(&self, id: &str) -> CliResult<DeleteOutcome> {
        self.delete(&format!("/pages/{}", encode_id(id))).await
    }

    pub async fn pages_batch_delete(
        &self,
        ids: Vec<String>,
    ) -> CliResult<(u16, BatchDeleteResponse)> {
        self.post_raw(
            "/pages/batch-delete",
            &BatchDeleteRequest { document_ids: ids },
        )
        .await
    }

    pub async fn search(&self, query: &str) -> CliResult<SearchResponse> {
        self.get(&format!("/search{}", qs(query))).await
    }

    pub async fn connectors(&self) -> CliResult<Vec<ConnectorSummary>> {
        self.get("/connectors").await
    }

    pub async fn connector(&self, cc_pair_id: i32, query: &str) -> CliResult<ConnectorDetail> {
        self.get(&format!("/connectors/{cc_pair_id}{}", qs(query)))
            .await
    }

    pub async fn connector_docs(
        &self,
        cc_pair_id: i32,
        query: &str,
    ) -> CliResult<ListResponse<PageListItem>> {
        self.get(&format!("/connectors/{cc_pair_id}/docs{}", qs(query)))
            .await
    }

    pub async fn connector_attempts(
        &self,
        cc_pair_id: i32,
        query: &str,
    ) -> CliResult<ListResponse<IndexAttemptItem>> {
        self.get(&format!("/connectors/{cc_pair_id}/attempts{}", qs(query)))
            .await
    }

    pub async fn connector_errors(
        &self,
        cc_pair_id: i32,
        query: &str,
    ) -> CliResult<IndexAttemptErrorsResponse> {
        self.get(&format!("/connectors/{cc_pair_id}/errors{}", qs(query)))
            .await
    }

    pub async fn connector_action(
        &self,
        cc_pair_id: i32,
        action: &str,
    ) -> CliResult<ActionResponse> {
        self.post(
            &format!("/connectors/{cc_pair_id}/{action}"),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn connector_run_once(
        &self,
        cc_pair_id: i32,
        request: &RunOnceRequest,
    ) -> CliResult<ActionResponse> {
        self.post(&format!("/connectors/{cc_pair_id}/run-once"), request)
            .await
    }

    pub async fn connector_delete(
        &self,
        cc_pair_id: i32,
        confirm_name: &str,
    ) -> CliResult<ActionResponse> {
        self.delete_with(
            &format!("/connectors/{cc_pair_id}"),
            &ConnectorDeleteRequest {
                confirm_name: confirm_name.to_string(),
            },
        )
        .await
    }

    pub async fn attempts(&self, query: &str) -> CliResult<ListResponse<IndexAttemptItem>> {
        self.get(&format!("/indexing/attempts{}", qs(query))).await
    }

    pub async fn stats_overview(&self) -> CliResult<StatsOverview> {
        self.get("/stats/overview").await
    }

    pub async fn stats_sources(&self) -> CliResult<Vec<SourceStat>> {
        self.get("/stats/sources").await
    }

    pub async fn stats_top_connectors(&self, query: &str) -> CliResult<Vec<TopConnector>> {
        self.get(&format!("/stats/connectors/top{}", qs(query)))
            .await
    }

    pub async fn stats_timeline(&self, query: &str) -> CliResult<TimelineResponse> {
        self.get(&format!("/stats/timeline{}", qs(query))).await
    }

    /// Health answers `503` when degraded, with a full body. Both statuses are
    /// real answers, so this returns the body either way and lets the caller
    /// decide the exit code.
    pub async fn health(&self) -> CliResult<(bool, HealthResponse)> {
        let response = self
            .send(self.request(reqwest::Method::GET, "/system/health"))
            .await?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| CliError::Other(anyhow::anyhow!("reading the response body: {e}")))?;

        match serde_json::from_slice::<HealthResponse>(&bytes) {
            Ok(report) => Ok((status == 200, report)),
            // A 401 or a proxy error page lands here; report it as what it is
            // rather than as a malformed health report.
            Err(_) => Err(error_from_body(status, &bytes)),
        }
    }

    pub async fn version(&self) -> CliResult<VersionResponse> {
        self.get("/system/version").await
    }

    /// Open the SSE stream behind `--all`.
    pub async fn stream(&self, query: &str) -> CliResult<reqwest::Response> {
        let response = self
            .send(self.request(reqwest::Method::GET, &format!("/pages/stream{}", qs(query))))
            .await?;
        let status = response.status();
        if !status.is_success() {
            let bytes = response.bytes().await.unwrap_or_default();
            return Err(error_from_body(status.as_u16(), &bytes));
        }
        Ok(response)
    }
}

/// Prefix a query string with `?` unless it is empty.
fn qs(query: &str) -> String {
    if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    }
}

/// Turn a non-2xx body into the richest error we can justify from it.
pub fn error_from_body(status: u16, body: &[u8]) -> CliError {
    match serde_json::from_slice::<ApiErrorEnvelope>(body) {
        Ok(envelope) => CliError::Api(envelope.error),
        Err(_) => CliError::Http {
            status,
            body: String::from_utf8_lossy(body).into_owned(),
        },
    }
}

/// reqwest nests its errors; the innermost message is the useful one.
fn root_cause(err: &reqwest::Error) -> String {
    let mut source: &dyn std::error::Error = err;
    while let Some(next) = source.source() {
        source = next;
    }
    source.to_string()
}

/// A tiny query-string builder. `serde_urlencoded` would do, but the CLI only
/// ever appends scalar pairs and this keeps the dependency list honest.
#[derive(Debug, Default, Clone)]
pub struct QueryBuilder {
    parts: Vec<String>,
}

impl QueryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, key: &str, value: impl std::fmt::Display) -> &mut Self {
        let encoded = utf8_percent_encode(&value.to_string(), PATH_SEGMENT).to_string();
        self.parts.push(format!("{key}={encoded}"));
        self
    }

    pub fn push_opt(&mut self, key: &str, value: Option<impl std::fmt::Display>) -> &mut Self {
        if let Some(value) = value {
            self.push(key, value);
        }
        self
    }

    pub fn build(&self) -> String {
        self.parts.join("&")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_ids_survive_as_exactly_one_path_segment() {
        // The backend requires this (05_AS_BUILT.md §2.3); an unencoded id gets a
        // 400 naming the fix, and the old CLI never encoded at all.
        assert_eq!(
            encode_id("https://example.com/a"),
            "https%3A%2F%2Fexample.com%2Fa"
        );
        assert!(!encode_id("https://example.com/a?b=1#c").contains('/'));
        assert!(!encode_id("https://example.com/a?b=1#c").contains('?'));
        assert!(!encode_id("https://example.com/a?b=1#c").contains('#'));
    }

    #[test]
    fn an_already_encoded_id_is_encoded_again_rather_than_passed_through() {
        // Double-encoding is the safe direction: the server decodes exactly once,
        // so a literal '%' in a URL must arrive as %25.
        assert_eq!(encode_id("a%2Fb"), "a%252Fb");
    }

    #[test]
    fn unicode_ids_are_encoded_as_utf8() {
        assert_eq!(encode_id("https://x/café"), "https%3A%2F%2Fx%2Fcaf%C3%A9");
    }

    #[test]
    fn the_error_envelope_is_preferred_over_the_raw_body() {
        let body = br#"{"error":{"code":"NOT_FOUND","message":"document 'x' not found","status":404,"req_id":"01J"}}"#;
        match error_from_body(404, body) {
            CliError::Api(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert_eq!(e.req_id, "01J");
            }
            other => panic!("expected an API error, got {other:?}"),
        }
    }

    #[test]
    fn a_non_envelope_body_is_reported_as_an_unexpected_shape() {
        // An HTML SPA fallback or a reverse-proxy error page.
        match error_from_body(502, b"<html>Bad Gateway</html>") {
            CliError::Http { status, .. } => assert_eq!(status, 502),
            other => panic!("expected an HTTP error, got {other:?}"),
        }
    }

    #[test]
    fn query_values_are_encoded_so_a_filter_with_an_ampersand_does_not_inject_a_parameter() {
        let mut q = QueryBuilder::new();
        q.push("search", "a&limit=9999").push("limit", 50);
        assert_eq!(q.build(), "search=a%26limit%3D9999&limit=50");
    }

    #[test]
    fn optional_parameters_are_omitted_rather_than_sent_empty() {
        let mut q = QueryBuilder::new();
        q.push_opt("source", None::<String>)
            .push_opt("connector_id", Some(4));
        assert_eq!(q.build(), "connector_id=4");
    }

    #[test]
    fn urls_are_built_under_the_versioned_prefix_with_no_double_slash() {
        let client = ApiClient::new("http://127.0.0.1:8080/", None, false).unwrap();
        assert_eq!(client.url("/pages"), "http://127.0.0.1:8080/api/v1/pages");
        assert_eq!(client.base_url(), "http://127.0.0.1:8080");
    }
}
