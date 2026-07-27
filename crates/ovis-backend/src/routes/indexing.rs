//! `/api/v1/indexing` — global crawl telemetry and the targeted-reindex flow.

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use ovis_core::api_types::{ListResponse, TargetedReindexRequest};
use serde::Deserialize;

use crate::error::AppError;
use crate::extract::{Json, Query};
use crate::state::AppState;

/// A targeted reindex is queued work on the Onyx side; keep a single request
/// from queueing an unbounded amount of it.
const MAX_REINDEX_TARGETS: usize = 1000;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AttemptsQuery {
    pub cc_pair_id: Option<i32>,
    /// Comma-separated, case-insensitive: `IN_PROGRESS,FAILED`.
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub page: Option<i64>,
}

pub async fn attempts(
    State(state): State<AppState>,
    Query(query): Query<AttemptsQuery>,
) -> Result<Response, AppError> {
    let limit = crate::services::pages::clamp_limit(query.limit, 50, 200);
    let page = query.page.unwrap_or(1).max(1);
    let offset = (page - 1) * limit;

    let statuses: Option<Vec<String>> = query.status.as_deref().map(|raw| {
        raw.split(',')
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .collect()
    });

    let (items, total) = tokio::try_join!(
        async {
            Ok::<_, AppError>(
                ovis_core::db::indexing::list_attempts(
                    &state.db,
                    query.cc_pair_id,
                    statuses.as_deref(),
                    limit,
                    offset,
                )
                .await?,
            )
        },
        async {
            Ok::<_, AppError>(
                ovis_core::db::indexing::count_attempts(
                    &state.db,
                    query.cc_pair_id,
                    statuses.as_deref(),
                )
                .await?,
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

pub async fn attempt(
    State(state): State<AppState>,
    Path(attempt_id): Path<i32>,
) -> Result<Response, AppError> {
    let attempt = ovis_core::db::indexing::get_attempt(&state.db, attempt_id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "index attempt",
            id: attempt_id.to_string(),
        })?;
    Ok(axum::Json(attempt).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BackgroundErrorsQuery {
    pub cc_pair_id: Option<i32>,
    pub limit: Option<i64>,
}

pub async fn background_errors(
    State(state): State<AppState>,
    Query(query): Query<BackgroundErrorsQuery>,
) -> Result<Response, AppError> {
    let limit = crate::services::pages::clamp_limit(query.limit, 50, 500);
    let items =
        ovis_core::db::indexing::list_background_errors(&state.db, query.cc_pair_id, limit).await?;
    Ok(axum::Json(items).into_response())
}

/// Onyx's own view of documents that failed to index.
pub async fn failed_documents(State(state): State<AppState>) -> Result<Response, AppError> {
    let onyx = state.onyx()?;
    Ok(axum::Json(onyx.failed_documents().await?).into_response())
}

/// Queue a reindex of specific documents, or of the recorded failures for a
/// cc-pair.
pub async fn targeted_reindex(
    State(state): State<AppState>,
    Json(body): Json<TargetedReindexRequest>,
) -> Result<Response, AppError> {
    let onyx = state.onyx()?;

    let only_failed = body.only_failed.unwrap_or(false);
    let explicit = body.document_ids.clone().unwrap_or_default();

    if only_failed && !explicit.is_empty() {
        return Err(AppError::BadRequest(
            "pass either only_failed or document_ids, not both".into(),
        ));
    }
    if !only_failed && explicit.is_empty() {
        return Err(AppError::BadRequest(
            "supply document_ids, or set only_failed to retry this cc-pair's recorded failures"
                .into(),
        ));
    }
    if explicit.len() > MAX_REINDEX_TARGETS {
        return Err(AppError::BadRequest(format!(
            "{} document ids exceeds the limit of {MAX_REINDEX_TARGETS}",
            explicit.len()
        )));
    }

    // Confirm the cc-pair exists before asking Onyx to do anything.
    let pair = ovis_core::db::connectors::get_cc_pair_ref(&state.db, body.cc_pair_id).await?;

    let response = if only_failed {
        let errors = ovis_core::db::indexing::list_attempt_errors(
            &state.db,
            Some(body.cc_pair_id),
            true,
            MAX_REINDEX_TARGETS as i64,
            0,
        )
        .await?;
        if errors.is_empty() {
            return Err(AppError::Conflict(format!(
                "cc-pair {} has no unresolved indexing failures in the last {} window",
                body.cc_pair_id,
                ovis_core::db::indexing::ATTEMPT_ERROR_WINDOW
            )));
        }
        let error_ids: Vec<i32> = errors.iter().map(|e| e.id).collect();
        tracing::info!(
            action = "targeted_reindex",
            cc_pair_id = body.cc_pair_id,
            connector = %pair.name,
            error_count = error_ids.len(),
            "queued a reindex of recorded failures"
        );
        onyx.targeted_reindex(Some(&error_ids), None).await?
    } else {
        let targets: Vec<(i32, String)> = explicit
            .iter()
            .map(|id| (body.cc_pair_id, id.clone()))
            .collect();
        tracing::info!(
            action = "targeted_reindex",
            cc_pair_id = body.cc_pair_id,
            connector = %pair.name,
            target_count = targets.len(),
            "queued a reindex of specific documents"
        );
        onyx.targeted_reindex(None, Some(&targets)).await?
    };

    Ok(axum::Json(response).into_response())
}

pub async fn targeted_reindex_status(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
) -> Result<Response, AppError> {
    let onyx = state.onyx()?;
    Ok(axum::Json(onyx.targeted_reindex_status(job_id).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_filters_are_split_trimmed_and_upper_cased() {
        let query = AttemptsQuery {
            status: Some(" in_progress , failed ,, ".into()),
            ..Default::default()
        };
        let statuses: Vec<String> = query
            .status
            .as_deref()
            .map(|raw| {
                raw.split(',')
                    .map(|s| s.trim().to_uppercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap();
        assert_eq!(statuses, vec!["IN_PROGRESS", "FAILED"]);
    }

    #[test]
    fn unknown_parameters_are_rejected() {
        assert!(serde_urlencoded::from_str::<AttemptsQuery>("statuses=FAILED").is_err());
        assert!(serde_urlencoded::from_str::<AttemptsQuery>("status=FAILED").is_ok());
        assert!(serde_urlencoded::from_str::<BackgroundErrorsQuery>("cc_pair=1").is_err());
    }

    #[test]
    fn reindex_requires_exactly_one_of_the_two_modes() {
        let both = TargetedReindexRequest {
            cc_pair_id: 1,
            document_ids: Some(vec!["a".into()]),
            only_failed: Some(true),
        };
        assert!(both.only_failed.unwrap() && !both.document_ids.unwrap().is_empty());

        let neither = TargetedReindexRequest {
            cc_pair_id: 1,
            document_ids: None,
            only_failed: None,
        };
        assert!(!neither.only_failed.unwrap_or(false));
        assert!(neither.document_ids.unwrap_or_default().is_empty());
    }

    #[test]
    fn reindex_targets_are_bounded() {
        assert_eq!(MAX_REINDEX_TARGETS, 1000);
    }
}
