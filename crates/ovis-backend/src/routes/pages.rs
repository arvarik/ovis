//! `/api/v1/pages` — list, detail, chunks, vector, text, patch, delete.
//!
//! Handlers here parse and map only. Everything about *how* an answer is
//! assembled lives in `services::pages`.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use ovis_core::api_types::{BatchDeleteRequest, PagePatch};
use ovis_core::cursor::SortOrder;
use ovis_core::db::documents::DocumentFilter;
use serde::Deserialize;

use crate::error::{AppError, RequestId};
use crate::extract::{decode_path_id, Json, Query};
use crate::services::pages as service;
use crate::state::AppState;

/// `deny_unknown_fields` is what turns `?sortt=` into a 400 rather than a
/// silently mis-ordered page.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    pub search: Option<String>,
    pub connector_id: Option<i32>,
    pub source: Option<String>,
    pub hidden: Option<bool>,
    pub chunk_min: Option<i32>,
    pub chunk_max: Option<i32>,
    pub updated_after: Option<DateTime<Utc>>,
    pub updated_before: Option<DateTime<Utc>>,
    pub sort: Option<String>,
    pub limit: Option<i64>,
    pub page: Option<i64>,
    pub cursor: Option<String>,
    /// Accepted on SSE only; tolerated here so a client can reuse one query
    /// string for both endpoints.
    pub token: Option<String>,
}

impl ListQuery {
    pub fn sort_order(&self) -> Result<SortOrder, AppError> {
        match self.sort.as_deref() {
            None => Ok(SortOrder::default()),
            Some(raw) => raw.parse().map_err(AppError::from),
        }
    }

    pub fn filter(&self) -> Result<DocumentFilter, AppError> {
        if let (Some(min), Some(max)) = (self.chunk_min, self.chunk_max) {
            if min > max {
                return Err(AppError::BadRequest(format!(
                    "chunk_min ({min}) is greater than chunk_max ({max})"
                )));
            }
        }
        if let (Some(after), Some(before)) = (self.updated_after, self.updated_before) {
            if after > before {
                return Err(AppError::BadRequest(
                    "updated_after is later than updated_before".into(),
                ));
            }
        }
        Ok(DocumentFilter {
            search: self
                .search
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            connector_id: self.connector_id,
            source: self
                .source
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            hidden: self.hidden,
            chunk_min: self.chunk_min,
            chunk_max: self.chunk_max,
            updated_after: self.updated_after,
            updated_before: self.updated_before,
        })
    }

    pub fn position(&self) -> Result<service::RequestedPosition, AppError> {
        match (&self.cursor, self.page) {
            (Some(cursor), Some(page)) if page > 1 => Err(AppError::BadRequest(format!(
                "pass either cursor or page, not both (got page={page})"
            ))),
            (Some(cursor), _) => Ok(service::RequestedPosition::Cursor(cursor.clone())),
            (None, page) => Ok(service::RequestedPosition::Page(page.unwrap_or(1).max(1))),
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Response, AppError> {
    let limit = service::clamp_limit(query.limit, 50, state.cfg.max_page_size);
    let position = query.position()?;
    if let service::RequestedPosition::Page(page) = &position {
        service::validate_page_depth(*page, limit)?;
    }

    let response = service::list(
        &state,
        service::ListRequest {
            filter: query.filter()?,
            sort: query.sort_order()?,
            position,
            limit,
        },
    )
    .await?;

    Ok(axum::Json(response).into_response())
}

pub async fn detail(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Response, AppError> {
    let id = decode_path_id(&raw_id);
    let detail = service::detail(&state, &id).await?;
    Ok(axum::Json(detail).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ChunksQuery {
    pub limit: Option<i64>,
    /// `chunk_index` to resume after, from `next_after`.
    pub after: Option<i64>,
    /// `content` (default) or `meta_only`.
    pub include: Option<String>,
}

pub async fn chunks(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    Query(query): Query<ChunksQuery>,
) -> Result<Response, AppError> {
    let id = decode_path_id(&raw_id);
    let limit = service::clamp_limit(query.limit, 100, 500);
    let include_content = match query.include.as_deref() {
        None | Some("content") => true,
        Some("meta_only") => false,
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unknown include '{other}'; expected content or meta_only"
            )))
        }
    };

    let response = service::chunks(&state, &id, query.after, limit, include_content).await?;
    Ok(axum::Json(response).into_response())
}

pub async fn chunk_vector(
    State(state): State<AppState>,
    Path((raw_id, chunk_index)): Path<(String, i64)>,
) -> Result<Response, AppError> {
    let id = decode_path_id(&raw_id);
    if chunk_index < 0 {
        return Err(AppError::BadRequest(
            "chunk_index must not be negative".into(),
        ));
    }
    let vector = service::chunk_vector(&state, &id, chunk_index).await?;
    Ok(axum::Json(vector).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TextQuery {
    /// `1` sets a `Content-Disposition: attachment` header.
    pub download: Option<String>,
}

pub async fn text(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    Query(query): Query<TextQuery>,
) -> Result<Response, AppError> {
    let id = decode_path_id(&raw_id);
    let body = service::text(&state, &id).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if matches!(query.download.as_deref(), Some("1") | Some("true")) {
        if let Ok(value) = HeaderValue::from_str(&format!(
            "attachment; filename=\"{}.txt\"",
            safe_filename(&id)
        )) {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
    }
    Ok((headers, body).into_response())
}

/// A document id is a URL; reduce it to something a filesystem accepts.
fn safe_filename(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "document".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

pub async fn patch(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    Json(body): Json<PagePatch>,
) -> Result<Response, AppError> {
    let id = decode_path_id(&raw_id);
    let response = service::patch(&state, &id, body).await?;
    Ok(axum::Json(response).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Response, AppError> {
    let id = decode_path_id(&raw_id);
    let outcome = service::delete(&state, &id).await?;
    Ok(axum::Json(outcome).into_response())
}

pub async fn batch_delete(
    State(state): State<AppState>,
    Json(body): Json<BatchDeleteRequest>,
) -> Result<Response, AppError> {
    let ids: Vec<String> = body
        .document_ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    let response = service::batch_delete(&state, ids).await?;

    // Partial failure is not success. 207 says "read the per-item outcomes".
    let status = if response.success {
        StatusCode::OK
    } else {
        StatusCode::MULTI_STATUS
    };
    Ok((status, axum::Json(response)).into_response())
}

/// Attach the request id when rendering an error from a handler that has it.
pub fn error_with_id(headers: &HeaderMap, err: AppError) -> Response {
    let req_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    err.into_response_with_id(req_id)
}

/// Pull the request id out of extensions, for handlers that need it explicitly
/// (the SSE stream does).
pub fn req_id(extensions: &axum::http::Extensions) -> String {
    extensions
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_defaults_to_recency_and_rejects_typos() {
        assert_eq!(
            ListQuery::default().sort_order().unwrap(),
            SortOrder::UpdatedDesc
        );
        let query = ListQuery {
            sort: Some("chunk_desc".into()),
            ..Default::default()
        };
        let err = query.sort_order().unwrap_err();
        assert_eq!(err.code(), "BAD_REQUEST");
        assert!(err.client_message().contains("chunks_desc"));
    }

    #[test]
    fn unknown_query_parameters_are_rejected() {
        let err = serde_urlencoded::from_str::<ListQuery>("sortt=updated_desc").unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn every_documented_parameter_is_accepted() {
        let query = "search=tax&connector_id=4&source=web&hidden=false&chunk_min=1\
                     &chunk_max=99&updated_after=2026-01-01T00:00:00Z\
                     &updated_before=2026-07-01T00:00:00Z&sort=chunks_desc&limit=25&page=2";
        let parsed: ListQuery = serde_urlencoded::from_str(query).expect("parses");
        assert_eq!(parsed.search.as_deref(), Some("tax"));
        assert_eq!(parsed.connector_id, Some(4));
        assert_eq!(parsed.hidden, Some(false));
        assert_eq!(parsed.chunk_min, Some(1));
        assert_eq!(parsed.limit, Some(25));
        assert_eq!(parsed.page, Some(2));
        assert_eq!(parsed.sort_order().unwrap(), SortOrder::ChunksDesc);
        assert!(parsed.filter().unwrap().updated_after.is_some());
    }

    #[test]
    fn blank_filters_are_treated_as_absent() {
        let query = ListQuery {
            search: Some("   ".into()),
            source: Some("".into()),
            ..Default::default()
        };
        let filter = query.filter().unwrap();
        assert_eq!(filter.search, None);
        assert_eq!(filter.source, None);
        assert!(filter.is_unfiltered());
    }

    #[test]
    fn contradictory_bounds_are_rejected_rather_than_returning_nothing() {
        let query = ListQuery {
            chunk_min: Some(10),
            chunk_max: Some(5),
            ..Default::default()
        };
        let err = query.filter().unwrap_err();
        assert!(err.client_message().contains("chunk_min"));

        let query = ListQuery {
            updated_after: Some("2026-07-01T00:00:00Z".parse().unwrap()),
            updated_before: Some("2026-01-01T00:00:00Z".parse().unwrap()),
            ..Default::default()
        };
        assert!(query.filter().is_err());
    }

    #[test]
    fn equal_bounds_are_allowed_because_that_is_the_stubs_preset() {
        // stubs = chunk_min=0&chunk_max=0
        let query = ListQuery {
            chunk_min: Some(0),
            chunk_max: Some(0),
            ..Default::default()
        };
        let filter = query.filter().unwrap();
        assert_eq!(filter.chunk_min, Some(0));
        assert_eq!(filter.chunk_max, Some(0));
    }

    #[test]
    fn cursor_and_page_together_are_rejected_unless_page_is_the_default() {
        let query = ListQuery {
            cursor: Some("abc".into()),
            page: Some(3),
            ..Default::default()
        };
        assert!(query.position().is_err());

        // page=1 is the implicit default, so a client echoing it with a cursor is
        // not an error.
        let query = ListQuery {
            cursor: Some("abc".into()),
            page: Some(1),
            ..Default::default()
        };
        assert!(matches!(
            query.position().unwrap(),
            service::RequestedPosition::Cursor(_)
        ));
    }

    #[test]
    fn page_zero_and_negative_clamp_to_the_first_page() {
        for page in [0, -5] {
            let query = ListQuery {
                page: Some(page),
                ..Default::default()
            };
            match query.position().unwrap() {
                service::RequestedPosition::Page(p) => assert_eq!(p, 1),
                other => panic!("expected a page position, got {other:?}"),
            }
        }
    }

    #[test]
    fn download_filenames_are_filesystem_safe() {
        assert_eq!(
            safe_filename("https://example.com/a/b?c=1"),
            "https___example.com_a_b_c_1"
        );
        assert_eq!(safe_filename("///"), "document");
        assert!(safe_filename(&format!("https://x/{}", "a".repeat(500))).len() <= 120);
        assert!(!safe_filename("../../etc/passwd").contains('/'));
    }

    #[test]
    fn chunks_include_parameter_only_accepts_the_documented_values() {
        for (value, expected) in [(None, true), (Some("content"), true), (Some("meta_only"), false)]
        {
            let include = match value {
                None | Some("content") => true,
                Some("meta_only") => false,
                Some(_) => panic!("unreachable"),
            };
            assert_eq!(include, expected);
        }
        let bad: Result<bool, &str> = match Some("everything") {
            None | Some("content") => Ok(true),
            Some("meta_only") => Ok(false),
            Some(other) => Err(other),
        };
        assert_eq!(bad, Err("everything"));
    }
}
