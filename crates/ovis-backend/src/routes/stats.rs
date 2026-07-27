//! `/api/v1/stats` — dashboard aggregates.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use ovis_core::db::stats::{TimelineBucketSize, TimelineWindow};
use serde::Deserialize;

use crate::error::AppError;
use crate::extract::Query;
use crate::services::stats as service;
use crate::state::AppState;

pub async fn overview(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok(axum::Json(service::overview(&state).await?).into_response())
}

pub async fn index(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok(axum::Json(service::index_stats(&state).await).into_response())
}

pub async fn sources(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok(axum::Json(service::by_source(&state).await?).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TopQuery {
    /// `docs` (default) or `recent`.
    pub by: Option<String>,
    pub limit: Option<i64>,
}

pub async fn top_connectors(
    State(state): State<AppState>,
    Query(query): Query<TopQuery>,
) -> Result<Response, AppError> {
    let by_recent = match query.by.as_deref() {
        None | Some("docs") => false,
        Some("recent") => true,
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unknown by '{other}'; expected docs or recent"
            )))
        }
    };
    let limit = crate::services::pages::clamp_limit(query.limit, 10, 200);
    Ok(axum::Json(service::top_connectors(&state, by_recent, limit).await?).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TimelineQuery {
    pub window: Option<String>,
    pub bucket: Option<String>,
}

pub async fn timeline(
    State(state): State<AppState>,
    Query(query): Query<TimelineQuery>,
) -> Result<Response, AppError> {
    let window: TimelineWindow = match query.window.as_deref() {
        None => TimelineWindow::Day,
        Some(raw) => raw.parse().map_err(AppError::from)?,
    };
    let bucket: TimelineBucketSize = match query.bucket.as_deref() {
        None => window.default_bucket(),
        Some(raw) => raw.parse().map_err(AppError::from)?,
    };
    Ok(axum::Json(service::timeline(&state, window, bucket).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_defaults_are_sensible_per_window() {
        assert_eq!(
            TimelineWindow::Day.default_bucket(),
            TimelineBucketSize::Hour
        );
        assert_eq!(
            TimelineWindow::Week.default_bucket(),
            TimelineBucketSize::Day
        );
    }

    #[test]
    fn unknown_windows_and_buckets_are_rejected_with_the_valid_set() {
        let err: AppError = "1y".parse::<TimelineWindow>().unwrap_err().into();
        assert!(err.client_message().contains("24h"));
        let err: AppError = "1w".parse::<TimelineBucketSize>().unwrap_err().into();
        assert!(err.client_message().contains("1h"));
    }

    #[test]
    fn unknown_parameters_are_rejected() {
        assert!(serde_urlencoded::from_str::<TimelineQuery>("windows=24h").is_err());
        assert!(serde_urlencoded::from_str::<TopQuery>("sort=docs").is_err());
        assert!(serde_urlencoded::from_str::<TopQuery>("by=docs&limit=5").is_ok());
    }
}
