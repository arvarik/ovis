//! Query embeddings from the vLLM endpoint behind the hppc nginx load balancer.
//!
//! Optional by design: when `EMBED_API_URL` is unset or the endpoint is down,
//! semantic and hybrid search fall back to BM25 and say so in the response
//! (`degraded: "no_embedder"`). A search that quietly returns nothing would be
//! worse than a search that admits it is running keyword-only.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone)]
pub struct EmbedClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl EmbedClient {
    pub fn new(base_url: &str, model: &str) -> CoreResult<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            // A search must not wait longer than this for an optional
            // enhancement; past the timeout we serve BM25.
            .timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(4)
            .build()
            .map_err(|e| CoreError::embed(format!("cannot build embedding client: {e}")))?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Embed one query string.
    ///
    /// `query_prefix` comes from `search_settings.query_prefix` — read from the
    /// database, not guessed. Embedding a query without the prefix its model was
    /// trained with silently degrades retrieval quality rather than failing, so
    /// it is worth taking from the source of truth. (On this deployment the
    /// arctic-embed row has an empty prefix.)
    pub async fn embed_query(&self, query_prefix: &str, query: &str) -> CoreResult<Vec<f32>> {
        let input = format!("{query_prefix}{query}");
        let body = json!({ "model": self.model, "input": [input] });

        let response = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::embed(format!("embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(CoreError::embed(format!(
                "embedding endpoint returned HTTP {status}: {}",
                text.chars().take(400).collect::<String>()
            )));
        }

        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|e| CoreError::embed(format!("malformed embedding response: {e}")))?;

        let vector: Vec<f32> = payload["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| CoreError::embed("embedding response has no data[0].embedding"))?
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();

        if vector.is_empty() {
            return Err(CoreError::embed("embedding response contained no floats"));
        }
        Ok(vector)
    }

    /// Liveness check for `/system/health`.
    pub async fn ping(&self) -> CoreResult<Duration> {
        let started = Instant::now();
        let response = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await
            .map_err(|e| CoreError::embed(format!("embedding ping failed: {e}")))?;
        if !response.status().is_success() {
            return Err(CoreError::embed(format!(
                "embedding endpoint returned HTTP {}",
                response.status()
            )));
        }
        Ok(started.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn sends_the_configured_model_and_prefixed_input() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(body_json(json!({
                "model": "snowflake-arctic-embed:m",
                "input": ["search_query: tax reform"]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "embedding": [0.1, 0.2, 0.3] }]
            })))
            .mount(&server)
            .await;

        let client = EmbedClient::new(&server.uri(), "snowflake-arctic-embed:m").unwrap();
        let vector = client
            .embed_query("search_query: ", "tax reform")
            .await
            .unwrap();
        assert_eq!(vector, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn an_empty_prefix_is_not_turned_into_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(body_json(json!({ "model": "m", "input": ["tax reform"] })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "data": [{ "embedding": [1.0] }] })),
            )
            .mount(&server)
            .await;

        let client = EmbedClient::new(&server.uri(), "m").unwrap();
        assert_eq!(
            client.embed_query("", "tax reform").await.unwrap(),
            vec![1.0]
        );
    }

    #[tokio::test]
    async fn upstream_failure_is_an_embed_error_so_search_can_degrade() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("no capacity"))
            .mount(&server)
            .await;

        let client = EmbedClient::new(&server.uri(), "m").unwrap();
        let err = client.embed_query("", "q").await.unwrap_err();
        assert!(matches!(err, CoreError::Embed(_)));
        assert_eq!(err.code(), "EMBED_UPSTREAM");
    }

    #[tokio::test]
    async fn a_response_without_floats_is_rejected_rather_than_returning_an_empty_vector() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "embedding": [] }] })),
            )
            .mount(&server)
            .await;

        let client = EmbedClient::new(&server.uri(), "m").unwrap();
        let err = client.embed_query("", "q").await.unwrap_err();
        assert!(err.to_string().contains("no floats"));
    }
}
