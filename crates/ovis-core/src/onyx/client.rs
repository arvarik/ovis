//! Onyx API client.
//!
//! ## Authentication
//!
//! The redesign called for minting an admin API key via `POST /admin/api-key`.
//! That endpoint is paywalled on this deployment — Onyx v4.3.4 answers
//! `402 FEATURE_NOT_AVAILABLE: This feature requires the Business plan` before
//! it even looks at credentials, and the `api_key` table is consequently empty.
//!
//! What *is* available on the free tier is a **Personal Access Token**
//! (`POST /user/pats`, gated only on `BASIC_ACCESS`). A PAT is presented the same
//! way an API key would be — `Authorization: Bearer …` — so `ONYX_API_KEY`
//! accepts either, and [`OnyxClient::mint_pat`] implements the one-time
//! login-and-mint flow for whichever the deployment supports.
//!
//! ## Retries
//!
//! Reads may be retried; **mutating calls never are**. A `run-once` that timed
//! out may well have started a crawl, and firing a second one would violate the
//! first-pass crawl policy this deployment depends on.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{CoreError, CoreResult};

/// How OVIS presents itself to Onyx.
#[derive(Clone)]
pub enum OnyxAuth {
    /// An API key or Personal Access Token.
    Bearer(String),
}

impl std::fmt::Debug for OnyxAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never let a token reach a log line through a derived Debug.
        f.write_str("OnyxAuth::Bearer(<redacted>)")
    }
}

/// Admin credentials used once, at setup time, to mint a token.
#[derive(Clone)]
pub struct PatCredentials {
    pub email: String,
    pub password: String,
    pub token_name: String,
}

impl std::fmt::Debug for PatCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatCredentials")
            .field("email", &self.email)
            .field("password", &"<redacted>")
            .field("token_name", &self.token_name)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OnyxVersion {
    pub backend_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OnyxClient {
    client: reqwest::Client,
    base_url: String,
    auth: OnyxAuth,
}

impl OnyxClient {
    pub fn new(base_url: &str, token: &str) -> CoreResult<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            // Some Onyx admin calls do real work before answering (prune kicks,
            // deletion attempts), hence the longer ceiling than OpenSearch gets.
            .timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(4)
            .build()
            .map_err(|e| {
                CoreError::Onyx {
                    status: 0,
                    body: format!("cannot build Onyx client: {e}"),
                }
            })?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            auth: OnyxAuth::Bearer(token.to_string()),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            OnyxAuth::Bearer(token) => req.bearer_auth(token),
        }
    }

    async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> CoreResult<Value> {
        let response = self
            .authed(req)
            .send()
            .await
            .map_err(|e| CoreError::Onyx {
                status: 0,
                body: format!("{what}: {e}"),
            })?;

        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();

        if !(200..300).contains(&status) {
            return Err(CoreError::Onyx {
                status,
                body: format!("{what}: {}", truncate(&body, 800)),
            });
        }

        if body.trim().is_empty() {
            // Several Onyx admin endpoints return 200 with no body.
            return Ok(Value::Null);
        }
        serde_json::from_str(&body).or(Ok(Value::String(truncate(&body, 400))))
    }

    async fn get(&self, path: &str, what: &str) -> CoreResult<Value> {
        let url = format!("{}{}", self.base_url, path);
        self.send(self.client.get(&url), what).await
    }

    async fn post(&self, path: &str, body: &Value, what: &str) -> CoreResult<Value> {
        let url = format!("{}{}", self.base_url, path);
        self.send(self.client.post(&url).json(body), what).await
    }

    async fn put(&self, path: &str, body: &Value, what: &str) -> CoreResult<Value> {
        let url = format!("{}{}", self.base_url, path);
        self.send(self.client.put(&url).json(body), what).await
    }

    async fn patch(&self, path: &str, body: &Value, what: &str) -> CoreResult<Value> {
        let url = format!("{}{}", self.base_url, path);
        self.send(self.client.patch(&url).json(body), what).await
    }

    async fn delete(&self, path: &str, what: &str) -> CoreResult<Value> {
        let url = format!("{}{}", self.base_url, path);
        self.send(self.client.delete(&url), what).await
    }

    // -----------------------------------------------------------------------
    // Health
    // -----------------------------------------------------------------------

    /// `/health` needs no auth, so this also works before a token is configured.
    pub async fn health(&self) -> CoreResult<Duration> {
        let started = Instant::now();
        let url = format!("{}/health", self.base_url);
        self.send(self.client.get(&url), "onyx health").await?;
        Ok(started.elapsed())
    }

    pub async fn version(&self) -> CoreResult<OnyxVersion> {
        let value = self.get("/version", "onyx version").await?;
        Ok(OnyxVersion {
            backend_version: value["backend_version"].as_str().map(|s| s.to_string()),
        })
    }

    /// Confirm the configured token is actually accepted. Used by
    /// `/system/health` so a stale token shows up as a health problem rather
    /// than as a 502 on the first action someone tries.
    pub async fn verify_token(&self) -> CoreResult<()> {
        self.get("/manage/admin/connector/status", "verify onyx token")
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------------

    /// Per-cc-pair status snapshot. This one is a POST despite being a read.
    pub async fn indexing_status(&self, secondary_index: bool) -> CoreResult<Value> {
        self.post(
            "/manage/admin/connector/indexing-status",
            &json!({ "secondary_index": secondary_index }),
            "fetch indexing status",
        )
        .await
    }

    pub async fn cc_pair(&self, cc_pair_id: i32) -> CoreResult<Value> {
        self.get(
            &format!("/manage/admin/cc-pair/{cc_pair_id}"),
            "fetch cc-pair",
        )
        .await
    }

    pub async fn cc_pair_index_attempts(
        &self,
        cc_pair_id: i32,
        page_num: i64,
        page_size: i64,
    ) -> CoreResult<Value> {
        self.get(
            &format!(
                "/manage/admin/cc-pair/{cc_pair_id}/index-attempts\
                 ?page_num={page_num}&page_size={page_size}"
            ),
            "fetch cc-pair index attempts",
        )
        .await
    }

    pub async fn failed_documents(&self) -> CoreResult<Value> {
        self.get(
            "/manage/admin/indexing/failed-documents",
            "fetch failed documents",
        )
        .await
    }

    pub async fn connector_docs(&self, cc_pair_id: i32) -> CoreResult<Value> {
        self.get(
            &format!("/onyx-api/connector-docs/{cc_pair_id}"),
            "fetch connector docs",
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Actions
    // -----------------------------------------------------------------------

    /// Pause or resume a cc-pair.
    pub async fn set_cc_pair_status(&self, cc_pair_id: i32, active: bool) -> CoreResult<Value> {
        let status = if active { "ACTIVE" } else { "PAUSED" };
        self.put(
            &format!("/manage/admin/cc-pair/{cc_pair_id}/status"),
            &json!({ "status": status }),
            if active { "resume cc-pair" } else { "pause cc-pair" },
        )
        .await
    }

    /// Trigger a crawl for **one** cc-pair.
    ///
    /// Deliberately per-pair: there is no bulk trigger anywhere in OVIS. Setting
    /// `indexing_trigger` across the ACTIVE web connectors at once is exactly
    /// what the first-pass crawl policy forbids.
    pub async fn run_once(
        &self,
        connector_id: i32,
        credential_id: i32,
        from_beginning: bool,
    ) -> CoreResult<Value> {
        self.post(
            "/manage/admin/connector/run-once",
            &json!({
                "connector_id": connector_id,
                "credential_ids": [credential_id],
                "from_beginning": from_beginning
            }),
            "trigger crawl",
        )
        .await
    }

    /// Kick Onyx's own prune for a cc-pair. (OVIS's separate pruning feature is
    /// out of scope; this is Onyx's.)
    pub async fn prune(&self, cc_pair_id: i32) -> CoreResult<Value> {
        self.post(
            &format!("/manage/admin/cc-pair/{cc_pair_id}/prune"),
            &Value::Null,
            "kick prune",
        )
        .await
    }

    /// Rename a cc-pair. Onyx takes the new name as a query parameter, not a body.
    pub async fn rename_cc_pair(&self, cc_pair_id: i32, new_name: &str) -> CoreResult<Value> {
        let encoded = urlencode(new_name);
        self.put(
            &format!("/manage/admin/cc-pair/{cc_pair_id}/name?new_name={encoded}"),
            &Value::Null,
            "rename cc-pair",
        )
        .await
    }

    pub async fn set_refresh_freq(&self, cc_pair_id: i32, seconds: i32) -> CoreResult<Value> {
        self.put(
            &format!("/manage/admin/cc-pair/{cc_pair_id}/property"),
            &json!({ "name": "refresh_frequency", "value": seconds.to_string() }),
            "set refresh frequency",
        )
        .await
    }

    /// Delete an entire cc-pair and every document it owns. The name-confirmation
    /// guard lives at the HTTP layer; by the time this is called the caller has
    /// already matched it.
    pub async fn delete_cc_pair(
        &self,
        connector_id: i32,
        credential_id: i32,
    ) -> CoreResult<Value> {
        self.post(
            "/manage/admin/deletion-attempt",
            &json!({ "connector_id": connector_id, "credential_id": credential_id }),
            "delete cc-pair",
        )
        .await
    }

    pub async fn patch_connector(&self, connector_id: i32, body: &Value) -> CoreResult<Value> {
        self.patch(
            &format!("/manage/admin/connector/{connector_id}"),
            body,
            "update connector",
        )
        .await
    }

    pub async fn delete_connector(&self, connector_id: i32) -> CoreResult<Value> {
        self.delete(
            &format!("/manage/admin/connector/{connector_id}"),
            "delete connector",
        )
        .await
    }

    /// Set a document's boost through Onyx, so Onyx syncs its own index.
    pub async fn set_doc_boost(&self, document_id: &str, boost: i32) -> CoreResult<Value> {
        self.post(
            "/manage/admin/doc-boosts",
            &json!({ "document_id": document_id, "boost": boost }),
            "set document boost",
        )
        .await
    }

    pub async fn set_doc_hidden(&self, document_id: &str, hidden: bool) -> CoreResult<Value> {
        self.post(
            "/manage/admin/doc-hidden",
            &json!({ "document_id": document_id, "hidden": hidden }),
            "set document hidden",
        )
        .await
    }

    /// Reindex specific documents, or retry a set of recorded failures.
    pub async fn targeted_reindex(
        &self,
        error_ids: Option<&[i32]>,
        targets: Option<&[(i32, String)]>,
    ) -> CoreResult<Value> {
        let mut body = serde_json::Map::new();
        if let Some(ids) = error_ids.filter(|i| !i.is_empty()) {
            body.insert("error_ids".into(), json!(ids));
        }
        if let Some(targets) = targets.filter(|t| !t.is_empty()) {
            body.insert(
                "targets".into(),
                json!(targets
                    .iter()
                    .map(|(cc_pair_id, document_id)| json!({
                        "cc_pair_id": cc_pair_id,
                        "document_id": document_id
                    }))
                    .collect::<Vec<Value>>()),
            );
        }
        if body.is_empty() {
            return Err(CoreError::Invalid(
                "targeted reindex needs either failed error ids or explicit document ids".into(),
            ));
        }
        self.post(
            "/manage/admin/indexing/targeted-reindex",
            &Value::Object(body),
            "submit targeted reindex",
        )
        .await
    }

    pub async fn targeted_reindex_status(&self, job_id: i64) -> CoreResult<Value> {
        self.get(
            &format!("/manage/admin/indexing/targeted-reindex/{job_id}"),
            "fetch targeted reindex status",
        )
        .await
    }

    // -----------------------------------------------------------------------
    // One-time token setup
    // -----------------------------------------------------------------------

    /// Log in with admin credentials and mint a token OVIS can use afterwards.
    ///
    /// Tries `POST /admin/api-key` first, since that is the documented mechanism,
    /// and falls back to `POST /user/pats` when the API-key feature is paywalled
    /// (which it is on the free tier). Returns the raw token — the caller stores
    /// it in `ONYX_API_KEY` and it is never persisted here.
    pub async fn mint_pat(base_url: &str, creds: &PatCredentials) -> CoreResult<String> {
        let base_url = base_url.trim_end_matches('/').to_string();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(30))
            // fastapi-users hands back a session cookie; keep it for the mint call.
            .cookie_store(true)
            .build()
            .map_err(|e| CoreError::Onyx {
                status: 0,
                body: format!("cannot build setup client: {e}"),
            })?;

        // fastapi-users' login route is form-encoded with OAuth2 field names.
        let login = client
            .post(format!("{base_url}/auth/login"))
            .form(&[
                ("username", creds.email.as_str()),
                ("password", creds.password.as_str()),
            ])
            .send()
            .await
            .map_err(|e| CoreError::Onyx {
                status: 0,
                body: format!("onyx login failed: {e}"),
            })?;

        let login_status = login.status().as_u16();
        if !(200..300).contains(&login_status) {
            return Err(CoreError::Onyx {
                status: login_status,
                body: "onyx login rejected the supplied admin credentials".into(),
            });
        }

        // Documented path first.
        let api_key = client
            .post(format!("{base_url}/admin/api-key"))
            .json(&json!({ "name": creds.token_name, "api_key_role": "admin" }))
            .send()
            .await;
        if let Ok(response) = api_key {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            if (200..300).contains(&status) {
                if let Some(key) = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v["api_key"].as_str().map(|s| s.to_string()))
                {
                    return Ok(key);
                }
            } else if status == 402 {
                tracing::info!(
                    "POST /admin/api-key is paywalled on this Onyx edition; \
                     minting a personal access token instead"
                );
            } else {
                tracing::warn!(
                    status,
                    "POST /admin/api-key failed; falling back to a personal access token"
                );
            }
        }

        // Free-tier path: an unscoped, non-expiring personal access token.
        let pat = client
            .post(format!("{base_url}/user/pats"))
            .json(&json!({
                "name": creds.token_name,
                "expiration_days": Value::Null,
                "scopes": Value::Null
            }))
            .send()
            .await
            .map_err(|e| CoreError::Onyx {
                status: 0,
                body: format!("minting a personal access token failed: {e}"),
            })?;

        let status = pat.status().as_u16();
        let body = pat.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(CoreError::Onyx {
                status,
                body: format!("minting a personal access token failed: {}", truncate(&body, 400)),
            });
        }

        let parsed: Value = serde_json::from_str(&body).map_err(|e| CoreError::Onyx {
            status,
            body: format!("malformed token response: {e}"),
        })?;

        parsed["token"]
            .as_str()
            .or_else(|| parsed["api_key"].as_str())
            .or_else(|| parsed["access_token"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| CoreError::Onyx {
                status,
                body: "token response contained no token field".into(),
            })
    }
}

fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
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
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn client(server: &MockServer) -> OnyxClient {
        OnyxClient::new(&server.uri(), "onyx_pat_secret").unwrap()
    }

    #[test]
    fn tokens_never_appear_in_debug_output() {
        let auth = OnyxAuth::Bearer("super-secret-token".into());
        assert!(!format!("{auth:?}").contains("super-secret-token"));

        let creds = PatCredentials {
            email: "admin@example.com".into(),
            password: "hunter2".into(),
            token_name: "ovis".into(),
        };
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("admin@example.com"));
    }

    #[tokio::test]
    async fn pause_and_resume_send_the_right_status() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/manage/admin/cc-pair/42/status"))
            .and(body_json(json!({ "status": "PAUSED" })))
            .and(header("authorization", "Bearer onyx_pat_secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "success": true })))
            .mount(&server)
            .await;

        client(&server).await.set_cc_pair_status(42, false).await.unwrap();

        let server2 = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/manage/admin/cc-pair/42/status"))
            .and(body_json(json!({ "status": "ACTIVE" })))
            .respond_with(ResponseTemplate::new(200).set_body_string(""))
            .mount(&server2)
            .await;

        client(&server2).await.set_cc_pair_status(42, true).await.unwrap();
    }

    #[tokio::test]
    async fn run_once_targets_exactly_one_credential() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/manage/admin/connector/run-once"))
            .and(body_json(json!({
                "connector_id": 7,
                "credential_ids": [3],
                "from_beginning": true
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "success": true })))
            .mount(&server)
            .await;

        client(&server).await.run_once(7, 3, true).await.unwrap();
    }

    #[tokio::test]
    async fn rename_puts_the_new_name_in_the_query_string() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/manage/admin/cc-pair/9/name"))
            .and(query_param("new_name", "a b/c"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "success": true })))
            .mount(&server)
            .await;

        client(&server).await.rename_cc_pair(9, "a b/c").await.unwrap();
    }

    #[tokio::test]
    async fn refresh_frequency_is_sent_as_a_stringified_property() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/manage/admin/cc-pair/9/property"))
            .and(body_json(json!({ "name": "refresh_frequency", "value": "2592000" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "success": true })))
            .mount(&server)
            .await;

        client(&server).await.set_refresh_freq(9, 2_592_000).await.unwrap();
    }

    #[tokio::test]
    async fn empty_success_bodies_are_not_treated_as_failures() {
        // POST /manage/admin/deletion-attempt returns 200 with no body.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/manage/admin/deletion-attempt"))
            .and(body_json(json!({ "connector_id": 1, "credential_id": 2 })))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        assert_eq!(
            client(&server).await.delete_cc_pair(1, 2).await.unwrap(),
            Value::Null
        );
    }

    #[tokio::test]
    async fn upstream_errors_carry_the_status_and_keep_the_body_out_of_display_reach() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Traceback: secrets here"))
            .mount(&server)
            .await;

        let err = client(&server).await.prune(1).await.unwrap_err();
        match &err {
            CoreError::Onyx { status, body } => {
                assert_eq!(*status, 500);
                // The body is available for logging...
                assert!(body.contains("Traceback"));
            }
            other => panic!("expected an Onyx error, got {other:?}"),
        }
        // ...and the wire code is the stable one the HTTP layer maps.
        assert_eq!(err.code(), "ONYX_UPSTREAM");
    }

    #[tokio::test]
    async fn connection_failure_is_an_onyx_error_with_status_zero() {
        // Nothing listening on this port.
        let onyx = OnyxClient::new("http://127.0.0.1:1", "token").unwrap();
        let err = onyx.prune(1).await.unwrap_err();
        assert!(matches!(err, CoreError::Onyx { status: 0, .. }));
    }

    #[tokio::test]
    async fn targeted_reindex_rejects_an_empty_request_before_calling_onyx() {
        let onyx = OnyxClient::new("http://127.0.0.1:1", "token").unwrap();
        let err = onyx.targeted_reindex(None, None).await.unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));

        let err = onyx.targeted_reindex(Some(&[]), Some(&[])).await.unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[tokio::test]
    async fn targeted_reindex_builds_both_request_shapes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/manage/admin/indexing/targeted-reindex"))
            .and(body_json(json!({ "error_ids": [1, 2, 3] })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "targeted_reindex_job_id": 5, "queued_count": 3 })),
            )
            .mount(&server)
            .await;
        let out = client(&server)
            .await
            .targeted_reindex(Some(&[1, 2, 3]), None)
            .await
            .unwrap();
        assert_eq!(out["targeted_reindex_job_id"], 5);

        let server2 = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_json(json!({
                "targets": [{ "cc_pair_id": 4, "document_id": "https://x/y" }]
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "targeted_reindex_job_id": 6 })),
            )
            .mount(&server2)
            .await;
        let out = client(&server2)
            .await
            .targeted_reindex(None, Some(&[(4, "https://x/y".to_string())]))
            .await
            .unwrap();
        assert_eq!(out["targeted_reindex_job_id"], 6);
    }

    #[tokio::test]
    async fn setup_falls_back_to_a_pat_when_api_keys_are_paywalled() {
        // Exactly what gamma does: 402 FEATURE_NOT_AVAILABLE on /admin/api-key.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/login"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/admin/api-key"))
            .respond_with(ResponseTemplate::new(402).set_body_json(json!({
                "error_code": "FEATURE_NOT_AVAILABLE",
                "detail": "This feature requires the Business plan."
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/user/pats"))
            .and(body_json(json!({
                "name": "ovis",
                "expiration_days": null,
                "scopes": null
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "token": "onyx_pat_abc123" })),
            )
            .mount(&server)
            .await;

        let token = OnyxClient::mint_pat(
            &server.uri(),
            &PatCredentials {
                email: "admin@example.com".into(),
                password: "pw".into(),
                token_name: "ovis".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(token, "onyx_pat_abc123");
    }

    #[tokio::test]
    async fn setup_prefers_an_api_key_when_the_edition_allows_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/login"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/admin/api-key"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "api_key": "on_key_xyz" })),
            )
            .mount(&server)
            .await;

        let token = OnyxClient::mint_pat(
            &server.uri(),
            &PatCredentials {
                email: "a@b.c".into(),
                password: "pw".into(),
                token_name: "ovis".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(token, "on_key_xyz");
    }

    #[tokio::test]
    async fn setup_reports_bad_admin_credentials_without_echoing_them() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/login"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "detail": "LOGIN_BAD_CREDENTIALS"
            })))
            .mount(&server)
            .await;

        let err = OnyxClient::mint_pat(
            &server.uri(),
            &PatCredentials {
                email: "a@b.c".into(),
                password: "wrong-password".into(),
                token_name: "ovis".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(!err.to_string().contains("wrong-password"));
        assert!(matches!(err, CoreError::Onyx { status: 400, .. }));
    }
}
