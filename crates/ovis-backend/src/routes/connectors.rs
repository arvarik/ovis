//! `/api/v1/connectors` — read side and the Onyx action proxy.

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use ovis_core::api_types::{
    ConnectorDeleteRequest, ConnectorPatchRequest, IndexAttemptErrorsResponse, ListResponse,
    RunOnceRequest,
};
use serde::Deserialize;

use crate::error::AppError;
use crate::extract::{Json, Query};
use crate::services::connectors as service;
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> Result<Response, AppError> {
    let summaries = service::summaries(&state).await?;
    Ok(axum::Json(summaries).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DetailQuery {
    /// `7d` / `30d` adds daily documents-added history.
    pub history: Option<String>,
}

pub async fn detail(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
    Query(query): Query<DetailQuery>,
) -> Result<Response, AppError> {
    let history_days = match query.history.as_deref() {
        None => None,
        Some(raw) => Some(parse_days(raw)?),
    };
    let detail = service::detail(&state, cc_pair_id, history_days).await?;
    Ok(axum::Json(detail).into_response())
}

/// Accept `7d`, `30d`, or a bare day count.
fn parse_days(raw: &str) -> Result<i64, AppError> {
    let trimmed = raw.trim().trim_end_matches('d');
    let days: i64 = trimmed.parse().map_err(|_| {
        AppError::BadRequest(format!(
            "history '{raw}' is not a day count; try 7d, 30d, or 90"
        ))
    })?;
    if !(1..=365).contains(&days) {
        return Err(AppError::BadRequest(
            "history must be between 1 and 365 days".into(),
        ));
    }
    Ok(days)
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PagedQuery {
    pub limit: Option<i64>,
    pub page: Option<i64>,
}

impl PagedQuery {
    fn resolve(&self, default_limit: i64, max_limit: i64) -> (i64, i64, i64) {
        let limit = crate::services::pages::clamp_limit(self.limit, default_limit, max_limit);
        let page = self.page.unwrap_or(1).max(1);
        (limit, page, (page - 1) * limit)
    }
}

pub async fn attempts(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
    Query(query): Query<PagedQuery>,
) -> Result<Response, AppError> {
    let (limit, page, offset) = query.resolve(50, 200);
    let (items, total) = tokio::try_join!(
        async {
            Ok::<_, AppError>(
                ovis_core::db::indexing::list_attempts(
                    &state.db,
                    Some(cc_pair_id),
                    None,
                    limit,
                    offset,
                )
                .await?,
            )
        },
        async {
            Ok::<_, AppError>(
                ovis_core::db::indexing::count_attempts(&state.db, Some(cc_pair_id), None).await?,
            )
        }
    )?;

    Ok(axum::Json(ListResponse {
        has_more: offset + (items.len() as i64) < total,
        items,
        total,
        total_exact: true,
        page: Some(page),
        limit,
        next_cursor: None,
    })
    .into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ErrorsQuery {
    pub limit: Option<i64>,
    pub page: Option<i64>,
    pub unresolved_only: Option<bool>,
}

pub async fn errors(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
    Query(query): Query<ErrorsQuery>,
) -> Result<Response, AppError> {
    let limit = crate::services::pages::clamp_limit(query.limit, 50, 200);
    let page = query.page.unwrap_or(1).max(1);
    let offset = (page - 1) * limit;
    let unresolved_only = query.unresolved_only.unwrap_or(false);

    let (items, total) = tokio::try_join!(
        async {
            Ok::<_, AppError>(
                ovis_core::db::indexing::list_attempt_errors(
                    &state.db,
                    Some(cc_pair_id),
                    unresolved_only,
                    limit,
                    offset,
                )
                .await?,
            )
        },
        async {
            Ok::<_, AppError>(
                ovis_core::db::indexing::count_attempt_errors(
                    &state.db,
                    Some(cc_pair_id),
                    unresolved_only,
                )
                .await?,
            )
        }
    )?;

    Ok(axum::Json(IndexAttemptErrorsResponse {
        has_more: offset + (items.len() as i64) < total,
        items,
        total,
        total_exact: true,
        page: Some(page),
        limit,
        next_cursor: None,
        // Pruned after 24 h by the resilience cron, so an empty list is not
        // evidence that nothing ever failed.
        window: ovis_core::db::indexing::ATTEMPT_ERROR_WINDOW.to_string(),
    })
    .into_response())
}

pub async fn docs(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
    Query(query): Query<PagedQuery>,
) -> Result<Response, AppError> {
    let (limit, page, offset) = query.resolve(50, state.cfg.max_page_size);
    crate::services::pages::validate_page_depth(page, limit)?;

    let (items, total) = tokio::try_join!(
        async {
            Ok::<_, AppError>(
                ovis_core::db::connectors::list_docs(&state.db, cc_pair_id, limit, offset).await?,
            )
        },
        async {
            Ok::<_, AppError>(ovis_core::db::connectors::count_docs(&state.db, cc_pair_id).await?)
        }
    )?;

    Ok(axum::Json(ListResponse {
        has_more: offset + (items.len() as i64) < total,
        items,
        total,
        total_exact: true,
        page: Some(page),
        limit,
        next_cursor: None,
    })
    .into_response())
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

pub async fn pause(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::pause(&state, cc_pair_id).await?).into_response())
}

pub async fn resume(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::resume(&state, cc_pair_id).await?).into_response())
}

/// A body is optional here: `POST /run-once` with nothing at all means "crawl
/// now, incrementally, and refuse if parked".
pub async fn run_once(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    let request: RunOnceRequest = if body.is_empty() {
        RunOnceRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| {
            AppError::BadRequest(format!(
                "invalid run-once body: {e}. Expected \
                 {{\"from_beginning\": bool, \"acknowledge_parked\": bool}} or no body at all."
            ))
        })?
    };
    Ok(axum::Json(service::run_once(&state, cc_pair_id, request).await?).into_response())
}

pub async fn prune(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::prune(&state, cc_pair_id).await?).into_response())
}

pub async fn patch(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
    Json(body): Json<ConnectorPatchRequest>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::patch(&state, cc_pair_id, body).await?).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    Path(cc_pair_id): Path<i32>,
    Json(body): Json<ConnectorDeleteRequest>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::delete(&state, cc_pair_id, &body.confirm_name).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_windows_accept_the_documented_and_the_obvious_forms() {
        assert_eq!(parse_days("7d").unwrap(), 7);
        assert_eq!(parse_days("30d").unwrap(), 30);
        assert_eq!(parse_days("90").unwrap(), 90);
        assert_eq!(parse_days(" 7d ").unwrap(), 7);
    }

    #[test]
    fn history_windows_are_bounded_and_typos_are_rejected() {
        assert!(parse_days("0").is_err());
        assert!(parse_days("366").is_err());
        assert!(parse_days("-1").is_err());
        assert!(parse_days("7w").is_err());
        assert!(parse_days("week").is_err());
    }

    #[test]
    fn paging_arithmetic_clamps_and_never_produces_a_negative_offset() {
        let (limit, page, offset) = PagedQuery {
            limit: Some(0),
            page: Some(0),
        }
        .resolve(50, 200);
        assert_eq!((limit, page, offset), (1, 1, 0));

        let (limit, page, offset) = PagedQuery {
            limit: Some(10_000),
            page: Some(3),
        }
        .resolve(50, 200);
        assert_eq!((limit, page, offset), (200, 3, 400));
    }

    #[test]
    fn unknown_parameters_are_rejected_on_every_connector_query_type() {
        assert!(serde_urlencoded::from_str::<DetailQuery>("historyy=7d").is_err());
        assert!(serde_urlencoded::from_str::<PagedQuery>("limitt=5").is_err());
        assert!(serde_urlencoded::from_str::<ErrorsQuery>("unresolved=1").is_err());
        assert!(serde_urlencoded::from_str::<ErrorsQuery>("unresolved_only=true").is_ok());
    }

    #[test]
    fn run_once_defaults_are_conservative() {
        // A bodyless POST must not mean "recrawl from the beginning" or "ignore
        // the park sentinel".
        let request = RunOnceRequest::default();
        assert!(!request.from_beginning);
        assert!(!request.acknowledge_parked);
    }
}
