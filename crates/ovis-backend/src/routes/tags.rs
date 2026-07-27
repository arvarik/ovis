//! `/api/v1/tags` — facet counts for filter pickers.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::AppError;
use crate::extract::Query;
use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TagsQuery {
    /// Narrow to one tag key, which is the "pick a value" step of a picker.
    pub key: Option<String>,
    /// Prefix-match the value, for type-ahead.
    pub value_prefix: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<TagsQuery>,
) -> Result<Response, AppError> {
    let limit = crate::services::pages::clamp_limit(query.limit, 100, 1000);
    let key = query
        .key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let prefix = query
        .value_prefix
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // Grouped scan over 445k rows, so always cached rather than run per
    // keystroke.
    let cache_key = format!("k={key:?}|p={prefix:?}|l={limit}");
    if let Some(cached) = state.caches.facets.get(&cache_key).await {
        return Ok(axum::Json(cached.as_ref().clone()).into_response());
    }

    let facets = Arc::new(ovis_core::db::tags::list_facets(&state.db, key, prefix, limit).await?);
    state
        .caches
        .facets
        .insert(cache_key, facets.clone())
        .await;

    Ok(axum::Json(facets.as_ref().clone()).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct KeysQuery {
    pub limit: Option<i64>,
}

pub async fn keys(
    State(state): State<AppState>,
    Query(query): Query<KeysQuery>,
) -> Result<Response, AppError> {
    let limit = crate::services::pages::clamp_limit(query.limit, 100, 1000);
    let keys = ovis_core::db::tags::list_keys(&state.db, limit).await?;
    let items: Vec<serde_json::Value> = keys
        .into_iter()
        .map(|(key, distinct_values)| {
            serde_json::json!({ "key": key, "distinct_values": distinct_values })
        })
        .collect();
    Ok(axum::Json(items).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_distinguish_the_query_shape() {
        let a = format!("k={:?}|p={:?}|l={}", Some("author"), None::<&str>, 100);
        let b = format!("k={:?}|p={:?}|l={}", Some("author"), Some("mar"), 100);
        let c = format!("k={:?}|p={:?}|l={}", Some("author"), None::<&str>, 50);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn unknown_parameters_are_rejected() {
        assert!(serde_urlencoded::from_str::<TagsQuery>("keys=author").is_err());
        assert!(serde_urlencoded::from_str::<TagsQuery>("key=author").is_ok());
        assert!(serde_urlencoded::from_str::<TagsQuery>("value_prefix=mar").is_ok());
    }
}
