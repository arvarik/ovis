//! `/api/v1/llm/*` — provider configuration, model discovery, capability
//! probing, and role assignment.
//!
//! Handlers parse and map only; the logic lives in [`crate::services::llm`].

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::AppError;
use crate::extract::{Json, Query};
use crate::services::llm as service;
use crate::state::AppState;

pub async fn list_providers(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok(axum::Json(serde_json::json!({
        "items": service::list_providers(&state).await?
    }))
    .into_response())
}

pub async fn create_provider(
    State(state): State<AppState>,
    Json(body): Json<service::CreateProviderRequest>,
) -> Result<Response, AppError> {
    let created = service::create_provider(&state, body).await?;
    Ok((axum::http::StatusCode::CREATED, axum::Json(created)).into_response())
}

pub async fn delete_provider(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    service::delete_provider(&state, id).await?;
    Ok(axum::Json(serde_json::json!({ "deleted": true })).into_response())
}

pub async fn rediscover(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let found = service::discover(&state, id).await?;
    Ok(axum::Json(serde_json::json!({ "models": found })).into_response())
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ModelsQuery {
    pub provider_id: Option<i64>,
}

pub async fn list_models(
    State(state): State<AppState>,
    Query(query): Query<ModelsQuery>,
) -> Result<Response, AppError> {
    Ok(axum::Json(serde_json::json!({
        "items": service::list_models(&state, query.provider_id).await?
    }))
    .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeRequest {
    pub model_id: String,
}

/// Probe one model.
///
/// The model id travels in the body rather than the path: llama.cpp reports a
/// filesystem path as its id, and a leading slash cannot survive a path
/// segment intact — an earlier version lost it to `decode_path_id` and stored
/// the probe result against an id that matched no row.
pub async fn probe_model(
    State(state): State<AppState>,
    Path(provider_id): Path<i64>,
    Json(body): Json<ProbeRequest>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::probe(&state, provider_id, &body.model_id).await?).into_response())
}

pub async fn probe_provider(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::probe_all(&state, id).await?).into_response())
}

pub async fn roles(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok(axum::Json(service::roles(&state).await?).into_response())
}

pub async fn assign_role(
    State(state): State<AppState>,
    Json(body): Json<service::AssignRoleRequest>,
) -> Result<Response, AppError> {
    Ok(axum::Json(service::assign_role(&state, body).await?).into_response())
}
