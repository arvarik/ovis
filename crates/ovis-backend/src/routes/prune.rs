//! `/api/v1/prune/*` — review, lifecycle, rules, scans, audit, exclusions.
//!
//! Handlers parse and map only; the safety logic lives in `services::prune`
//! (and deletion itself only in `services::prune_reaper`).

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use ovis_core::api_types::{
    PruneDismissRequest, PruneRestoreRequest, PruneRuleCreate, PruneRulePatch, PruneScanRequest,
    PruneScheduleDeleteRequest, PruneStageRequest,
};
use ovis_core::db::prune as db;
use serde::Deserialize;

use crate::error::AppError;
use crate::extract::{decode_path_id, Json, Query};
use crate::services::pages as pages_service;
use crate::services::prune as service;
use crate::services::prune_triage as triage;
use crate::services::trash as trash_service;
use crate::state::AppState;

pub async fn status(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok(axum::Json(service::status(&state).await?).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CandidatesQuery {
    pub state: Option<String>,
    pub detector: Option<String>,
    pub connector_id: Option<i32>,
    pub min_confidence: Option<f32>,
    pub recrawl_risk: Option<bool>,
    pub scan_id: Option<i64>,
    pub sort: Option<String>,
    pub limit: Option<i64>,
    pub page: Option<i64>,
}

pub async fn candidates(
    State(state): State<AppState>,
    Query(query): Query<CandidatesQuery>,
) -> Result<Response, AppError> {
    let filter = service::filter_from_body(&ovis_core::api_types::PruneCandidateFilterBody {
        state: query.state.clone(),
        detector: query.detector.clone(),
        connector_id: query.connector_id,
        min_confidence: query.min_confidence,
        recrawl_risk: query.recrawl_risk,
        scan_id: query.scan_id,
    })?;
    let sort = match query.sort.as_deref() {
        None => db::CandidateSort::default(),
        Some(raw) => db::CandidateSort::parse(raw)?,
    };
    let limit = pages_service::clamp_limit(query.limit, 50, state.cfg.max_page_size);
    let page = query.page.unwrap_or(1).max(1);
    let response = service::list_candidates(&state, filter, sort, limit, page).await?;
    Ok(axum::Json(response).into_response())
}

pub async fn candidate_detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::candidate_detail(&state, id).await?).into_response())
}

pub async fn stage(
    State(state): State<AppState>,
    Json(body): Json<PruneStageRequest>,
) -> Result<Response, AppError> {
    respond_bulk(service::stage(&state, body).await?)
}

pub async fn dismiss(
    State(state): State<AppState>,
    Json(body): Json<PruneDismissRequest>,
) -> Result<Response, AppError> {
    respond_bulk(service::dismiss(&state, body).await?)
}

pub async fn restore(
    State(state): State<AppState>,
    Json(body): Json<PruneRestoreRequest>,
) -> Result<Response, AppError> {
    respond_bulk(service::restore(&state, body).await?)
}

pub async fn schedule_delete(
    State(state): State<AppState>,
    Json(body): Json<PruneScheduleDeleteRequest>,
) -> Result<Response, AppError> {
    respond_bulk(service::schedule_delete(&state, body).await?)
}

/// Partial failure is not success — 207, like batch delete.
fn respond_bulk(response: ovis_core::api_types::PruneBulkResponse) -> Result<Response, AppError> {
    let status = if response.success {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::MULTI_STATUS
    };
    Ok((status, axum::Json(response)).into_response())
}

// ---------------------------------------------------------------------------
// Scans
// ---------------------------------------------------------------------------

pub async fn create_scan(
    State(state): State<AppState>,
    Json(body): Json<PruneScanRequest>,
) -> Result<Response, AppError> {
    let scan = service::create_scan(&state, body).await?;
    Ok((axum::http::StatusCode::ACCEPTED, axum::Json(scan)).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    pub limit: Option<i64>,
    pub page: Option<i64>,
}

pub async fn list_scans(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let limit = pages_service::clamp_limit(query.limit, 20, state.cfg.max_page_size);
    let page = query.page.unwrap_or(1).max(1);
    Ok(axum::Json(service::list_scans(&state, limit, page).await?).into_response())
}

pub async fn scan_detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::get_scan(&state, id).await?).into_response())
}

pub async fn cancel_scan(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::cancel_scan(&state, id).await?).into_response())
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

pub async fn list_rules(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok(axum::Json(service::list_rules(&state).await?).into_response())
}

pub async fn create_rule(
    State(state): State<AppState>,
    Json(body): Json<PruneRuleCreate>,
) -> Result<Response, AppError> {
    let rule = service::create_rule(&state, body).await?;
    Ok((axum::http::StatusCode::CREATED, axum::Json(rule)).into_response())
}

pub async fn patch_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<PruneRulePatch>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::patch_rule(&state, id, body).await?).into_response())
}

pub async fn delete_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    service::delete_rule(&state, id).await?;
    Ok(axum::Json(serde_json::json!({ "deleted": true, "id": id })).into_response())
}

pub async fn preview_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::preview_rule(&state, id).await?).into_response())
}

// ---------------------------------------------------------------------------
// Config export / import (the YAML round-trip)
// ---------------------------------------------------------------------------

pub async fn export_config(State(state): State<AppState>) -> Result<Response, AppError> {
    let yaml = service::export_config(&state).await?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/yaml")],
        yaml,
    )
        .into_response())
}

pub async fn import_config(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    let yaml = std::str::from_utf8(&body)
        .map_err(|_| AppError::BadRequest("config body must be UTF-8 YAML".into()))?;
    if yaml.trim().is_empty() {
        return Err(AppError::BadRequest("config body is empty".into()));
    }
    let rule = service::import_config(&state, yaml).await?;
    Ok(axum::Json(rule).into_response())
}

// ---------------------------------------------------------------------------
// Audit & exclusions
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AuditQuery {
    pub action: Option<String>,
    pub actor: Option<String>,
    pub document_id: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub page: Option<i64>,
}

pub async fn audit(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> Result<Response, AppError> {
    let limit = pages_service::clamp_limit(query.limit, 50, state.cfg.max_page_size);
    let page = query.page.unwrap_or(1).max(1);
    let filter = db::AuditFilter {
        action: query.action,
        actor: query.actor,
        document_id: query.document_id,
        since: query.since,
    };
    Ok(axum::Json(service::list_audit(&state, filter, limit, page).await?).into_response())
}

pub async fn exclusions(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let limit = pages_service::clamp_limit(query.limit, 50, state.cfg.max_page_size);
    let page = query.page.unwrap_or(1).max(1);
    Ok(axum::Json(service::list_exclusions(&state, limit, page).await?).into_response())
}

pub async fn delete_exclusion(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Response, AppError> {
    let id = decode_path_id(&raw_id);
    service::delete_exclusion(&state, &id).await?;
    Ok(axum::Json(serde_json::json!({ "deleted": true, "document_id": id })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_candidate_query_parameters_are_rejected() {
        let err =
            serde_urlencoded::from_str::<CandidatesQuery>("min_confidance=0.5").unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn every_documented_candidate_parameter_parses() {
        let parsed: CandidatesQuery = serde_urlencoded::from_str(
            "state=staged&detector=thin&connector_id=4&min_confidence=0.8&recrawl_risk=true\
             &scan_id=3&sort=confidence_desc&limit=25&page=2",
        )
        .expect("parses");
        assert_eq!(parsed.state.as_deref(), Some("staged"));
        assert_eq!(parsed.detector.as_deref(), Some("thin"));
        assert_eq!(parsed.connector_id, Some(4));
        assert_eq!(parsed.min_confidence, Some(0.8));
        assert_eq!(parsed.recrawl_risk, Some(true));
        assert_eq!(parsed.scan_id, Some(3));
        assert_eq!(parsed.limit, Some(25));
        assert_eq!(parsed.page, Some(2));
    }
}

// ---------------------------------------------------------------------------
// v2 — triage, policy, clusters, sampling, trash
// ---------------------------------------------------------------------------

pub async fn overview(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok(axum::Json(triage::overview(&state).await?).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HistogramQuery {
    pub signal: String,
    pub buckets: Option<i64>,
}

pub async fn histogram(
    State(state): State<AppState>,
    Query(query): Query<HistogramQuery>,
) -> Result<Response, AppError> {
    service::guard(&state)?;
    let buckets = query.buckets.unwrap_or(20);
    let buckets = ovis_core::db::profile::histogram(&state.db, &query.signal, buckets).await?;
    Ok(axum::Json(serde_json::json!({
        "signal": query.signal,
        "buckets": buckets,
    }))
    .into_response())
}

pub async fn simulate(
    State(state): State<AppState>,
    Json(body): Json<triage::SimulateRequest>,
) -> Result<Response, AppError> {
    Ok(axum::Json(triage::simulate(&state, body).await?).into_response())
}

pub async fn commit_policy(
    State(state): State<AppState>,
    Json(body): Json<triage::CommitRequest>,
) -> Result<Response, AppError> {
    Ok(axum::Json(triage::commit(&state, body).await?).into_response())
}

pub async fn list_policies(State(state): State<AppState>) -> Result<Response, AppError> {
    service::guard(&state)?;
    let policies = ovis_core::db::profile::list_policies(&state.db).await?;
    Ok(axum::Json(serde_json::json!({ "items": policies })).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClustersQuery {
    pub method: Option<String>,
    pub after: Option<String>,
    pub limit: Option<i64>,
}

pub async fn clusters(
    State(state): State<AppState>,
    Query(query): Query<ClustersQuery>,
) -> Result<Response, AppError> {
    let clusters = triage::clusters(
        &state,
        query.method.as_deref(),
        query.after.as_deref(),
        query.limit.unwrap_or(20),
    )
    .await?;
    Ok(axum::Json(serde_json::json!({ "items": clusters })).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SampleQuery {
    pub detector: Option<String>,
    pub code: Option<String>,
    pub n: Option<i64>,
}

pub async fn sample(
    State(state): State<AppState>,
    Query(query): Query<SampleQuery>,
) -> Result<Response, AppError> {
    let plan = triage::sample(
        &state,
        query.detector.as_deref(),
        query.code.as_deref(),
        query.n.unwrap_or(60),
    )
    .await?;
    Ok(axum::Json(plan).into_response())
}

// --- trash ---

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TrashQuery {
    pub connector_id: Option<i32>,
    pub document_id: Option<String>,
    pub hold: Option<bool>,
    pub expiring_within_days: Option<i64>,
    pub limit: Option<i64>,
    pub page: Option<i64>,
}

pub async fn trash_list(
    State(state): State<AppState>,
    Query(query): Query<TrashQuery>,
) -> Result<Response, AppError> {
    Ok(axum::Json(trash_service::list(&state, query).await?).into_response())
}

pub async fn trash_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let id = decode_path_id(&id);
    Ok(axum::Json(trash_service::detail(&state, &id).await?).into_response())
}

pub async fn trash_restore(
    State(state): State<AppState>,
    Json(body): Json<trash_service::TrashBulkRequest>,
) -> Result<Response, AppError> {
    let response = trash_service::restore(&state, body).await?;
    let status = if response.success {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::MULTI_STATUS
    };
    Ok((status, axum::Json(response)).into_response())
}

pub async fn trash_purge(
    State(state): State<AppState>,
    Json(body): Json<trash_service::TrashBulkRequest>,
) -> Result<Response, AppError> {
    Ok(axum::Json(trash_service::purge(&state, body).await?).into_response())
}

pub async fn trash_hold(
    State(state): State<AppState>,
    Json(body): Json<trash_service::TrashHoldRequest>,
) -> Result<Response, AppError> {
    Ok(axum::Json(trash_service::set_hold(&state, body).await?).into_response())
}
