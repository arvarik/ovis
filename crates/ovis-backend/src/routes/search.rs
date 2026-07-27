//! `GET /api/v1/search` — content search over the chunk index.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use ovis_core::api_types::SearchMode;
use serde::Deserialize;

use crate::error::AppError;
use crate::extract::Query;
use crate::services::search as service;
use crate::state::AppState;

const MAX_LIMIT: i64 = 100;
/// Deep paging into a collapsed result set is expensive on the OpenSearch side
/// and never a real workflow; refine the query instead.
const MAX_OFFSET: i64 = 1000;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    pub q: Option<String>,
    pub mode: Option<String>,
    pub connector_id: Option<i32>,
    pub source: Option<String>,
    pub include_hidden: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl SearchParams {
    fn mode(&self) -> Result<SearchMode, AppError> {
        match self.mode.as_deref() {
            None | Some("keyword") => Ok(SearchMode::Keyword),
            Some("semantic") => Ok(SearchMode::Semantic),
            Some("hybrid") => Ok(SearchMode::Hybrid),
            Some(other) => Err(AppError::BadRequest(format!(
                "unknown mode '{other}'; expected keyword, semantic or hybrid"
            ))),
        }
    }
}

pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Response, AppError> {
    let q = params
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("q is required and must not be empty".into()))?
        .to_string();

    let offset = params.offset.unwrap_or(0);
    if offset < 0 {
        return Err(AppError::BadRequest("offset must not be negative".into()));
    }
    if offset > MAX_OFFSET {
        return Err(AppError::BadRequest(format!(
            "offset {offset} exceeds the maximum of {MAX_OFFSET}; refine the query instead"
        )));
    }

    let response = service::search(
        &state,
        service::SearchQuery {
            q,
            mode: params.mode()?,
            connector_id: params.connector_id,
            source: params
                .source
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            include_hidden: params.include_hidden.unwrap_or(false),
            limit: crate::services::pages::clamp_limit(params.limit, 20, MAX_LIMIT),
            offset,
        },
    )
    .await?;

    Ok(axum::Json(response).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_defaults_to_keyword_and_rejects_anything_else() {
        assert_eq!(SearchParams::default().mode().unwrap(), SearchMode::Keyword);
        for (raw, expected) in [
            ("keyword", SearchMode::Keyword),
            ("semantic", SearchMode::Semantic),
            ("hybrid", SearchMode::Hybrid),
        ] {
            let params = SearchParams {
                mode: Some(raw.into()),
                ..Default::default()
            };
            assert_eq!(params.mode().unwrap(), expected);
        }
        let params = SearchParams {
            mode: Some("vector".into()),
            ..Default::default()
        };
        assert!(params
            .mode()
            .unwrap_err()
            .client_message()
            .contains("hybrid"));
    }

    #[test]
    fn unknown_search_parameters_are_rejected() {
        assert!(serde_urlencoded::from_str::<SearchParams>("q=x&moode=hybrid").is_err());
        assert!(serde_urlencoded::from_str::<SearchParams>("q=x&mode=hybrid").is_ok());
    }

    #[test]
    fn offsets_and_limits_are_bounded() {
        // Deep paging into a collapsed result set is expensive on the OpenSearch
        // side and is never a real workflow; refine the query instead.
        assert_eq!(MAX_OFFSET, 1000);
        assert_eq!(MAX_LIMIT, 100);
    }
}
