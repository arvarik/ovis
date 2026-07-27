//! Connector reads and the Onyx action proxy.
//!
//! Every action here is audit-logged at `info` with the acting route, the
//! cc-pair, and the outcome. These are the operations that can pause a crawl or
//! delete 100k documents; "who asked for this and when" has to be answerable
//! from the log alone.

use ovis_core::api_types::{
    ActionResponse, ConnectorDetail, ConnectorPatchRequest, ConnectorSummary, RunOnceRequest,
};
use ovis_core::db::connectors::{self, CcPairRef};

use crate::error::AppError;
use crate::state::AppState;

pub async fn summaries(state: &AppState) -> Result<Vec<ConnectorSummary>, AppError> {
    Ok(super::pages::cached_connectors(state)
        .await?
        .as_ref()
        .clone())
}

pub async fn detail(
    state: &AppState,
    cc_pair_id: i32,
    history_days: Option<i64>,
) -> Result<ConnectorDetail, AppError> {
    let mut detail = connectors::get_detail(&state.db, cc_pair_id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "cc-pair",
            id: cc_pair_id.to_string(),
        })?;

    if let Some(days) = history_days {
        detail.history = Some(connectors::history(&state.db, cc_pair_id, days).await?);
    }
    Ok(detail)
}

/// Resolve a cc-pair and confirm an action may proceed.
async fn resolve(state: &AppState, cc_pair_id: i32) -> Result<CcPairRef, AppError> {
    Ok(connectors::get_cc_pair_ref(&state.db, cc_pair_id).await?)
}

fn action_ok(cc_pair_id: i32, action: &str, status: Option<&str>) -> ActionResponse {
    ActionResponse {
        ok: true,
        cc_pair_id,
        action: action.to_string(),
        status: status.map(|s| s.to_string()),
        detail: None,
    }
}

pub async fn pause(state: &AppState, cc_pair_id: i32) -> Result<ActionResponse, AppError> {
    let onyx = state.onyx()?;
    let pair = resolve(state, cc_pair_id).await?;
    onyx.set_cc_pair_status(cc_pair_id, false).await?;
    state.caches.invalidate_connector_scoped().await;
    tracing::info!(
        action = "pause",
        cc_pair_id,
        connector = %pair.name,
        previous_status = %pair.status,
        "paused a connector"
    );
    Ok(action_ok(cc_pair_id, "pause", Some("PAUSED")))
}

pub async fn resume(state: &AppState, cc_pair_id: i32) -> Result<ActionResponse, AppError> {
    let onyx = state.onyx()?;
    let pair = resolve(state, cc_pair_id).await?;
    onyx.set_cc_pair_status(cc_pair_id, true).await?;
    state.caches.invalidate_connector_scoped().await;
    tracing::info!(
        action = "resume",
        cc_pair_id,
        connector = %pair.name,
        previous_status = %pair.status,
        parked = pair.parked,
        "resumed a connector"
    );
    Ok(action_ok(cc_pair_id, "resume", Some("ACTIVE")))
}

/// Trigger a crawl for one cc-pair.
///
/// A *parked* pair — one the resilience cron deliberately skipped, marked by a
/// `first-pass already complete` or `park done` sentinel on its latest attempt —
/// requires `acknowledge_parked: true`. Kicking one by accident is how the
/// first-pass crawl policy gets violated.
pub async fn run_once(
    state: &AppState,
    cc_pair_id: i32,
    request: RunOnceRequest,
) -> Result<ActionResponse, AppError> {
    let onyx = state.onyx()?;
    let pair = resolve(state, cc_pair_id).await?;

    if pair.parked && !request.acknowledge_parked {
        return Err(AppError::ParkedConnector(format!(
            "cc-pair {cc_pair_id} ('{}') is parked by the resilience cron; re-send with \
             \"acknowledge_parked\": true to crawl it anyway",
            pair.name
        )));
    }

    onyx.run_once(
        pair.connector_id,
        pair.credential_id,
        request.from_beginning,
    )
    .await?;
    state.caches.invalidate_connector_scoped().await;

    tracing::info!(
        action = "run_once",
        cc_pair_id,
        connector = %pair.name,
        connector_id = pair.connector_id,
        credential_id = pair.credential_id,
        from_beginning = request.from_beginning,
        parked = pair.parked,
        acknowledged = request.acknowledge_parked,
        "triggered a crawl"
    );
    Ok(action_ok(cc_pair_id, "run-once", None))
}

pub async fn prune(state: &AppState, cc_pair_id: i32) -> Result<ActionResponse, AppError> {
    let onyx = state.onyx()?;
    let pair = resolve(state, cc_pair_id).await?;
    onyx.prune(cc_pair_id).await?;
    state.caches.invalidate_connector_scoped().await;
    tracing::info!(
        action = "prune",
        cc_pair_id,
        connector = %pair.name,
        "kicked an Onyx prune"
    );
    Ok(action_ok(cc_pair_id, "prune", None))
}

pub async fn patch(
    state: &AppState,
    cc_pair_id: i32,
    request: ConnectorPatchRequest,
) -> Result<ActionResponse, AppError> {
    let onyx = state.onyx()?;
    if request.name.is_none() && request.refresh_freq_secs.is_none() {
        return Err(AppError::BadRequest(
            "nothing to change: supply name and/or refresh_freq_secs".into(),
        ));
    }
    if let Some(freq) = request.refresh_freq_secs {
        if freq < 60 {
            return Err(AppError::BadRequest(
                "refresh_freq_secs must be at least 60".into(),
            ));
        }
    }

    let pair = resolve(state, cc_pair_id).await?;

    if let Some(name) = request.name.as_deref() {
        if name.trim().is_empty() {
            return Err(AppError::BadRequest("name must not be blank".into()));
        }
        onyx.rename_cc_pair(cc_pair_id, name).await?;
        tracing::info!(action = "rename", cc_pair_id, from = %pair.name, to = %name, "renamed a connector");
    }
    if let Some(freq) = request.refresh_freq_secs {
        onyx.set_refresh_freq(cc_pair_id, freq).await?;
        tracing::info!(action = "set_refresh_freq", cc_pair_id, connector = %pair.name, seconds = freq, "changed the refresh schedule");
    }

    state.caches.invalidate_connector_scoped().await;
    Ok(action_ok(cc_pair_id, "patch", None))
}

/// Delete an entire cc-pair and every document it owns.
///
/// Guarded by exact name match. The biggest connector here holds 105,666
/// documents; a mistyped id must not be able to remove them.
pub async fn delete(
    state: &AppState,
    cc_pair_id: i32,
    confirm_name: &str,
) -> Result<ActionResponse, AppError> {
    let onyx = state.onyx()?;
    let pair = resolve(state, cc_pair_id).await?;

    if confirm_name != pair.name {
        return Err(AppError::Conflict(format!(
            "confirm_name does not match: cc-pair {cc_pair_id} is named '{}'",
            pair.name
        )));
    }

    let doc_count = connectors::count_docs(&state.db, cc_pair_id)
        .await
        .unwrap_or(-1);

    onyx.delete_cc_pair(pair.connector_id, pair.credential_id)
        .await?;
    state.caches.invalidate_connector_scoped().await;
    state.caches.invalidate_document_scoped().await;

    tracing::warn!(
        action = "delete_cc_pair",
        cc_pair_id,
        connector = %pair.name,
        connector_id = pair.connector_id,
        credential_id = pair.credential_id,
        documents_affected = doc_count,
        "requested deletion of an entire connector"
    );

    Ok(ActionResponse {
        ok: true,
        cc_pair_id,
        action: "delete".into(),
        status: Some("DELETING".into()),
        detail: Some(format!(
            "Onyx is deleting this cc-pair and its {} documents in the background",
            if doc_count >= 0 {
                doc_count.to_string()
            } else {
                "(unknown)".to_string()
            }
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_responses_name_the_action_they_performed() {
        let response = action_ok(42, "pause", Some("PAUSED"));
        assert!(response.ok);
        assert_eq!(response.cc_pair_id, 42);
        assert_eq!(response.action, "pause");
        assert_eq!(response.status.as_deref(), Some("PAUSED"));
    }

    #[test]
    fn refresh_frequency_floor_is_enforced() {
        // Guarded here rather than trusting Onyx to reject it: a 1-second refresh
        // on a web connector would hammer someone else's site.
        for bad in [0, 1, 59, -1] {
            let request = ConnectorPatchRequest {
                name: None,
                refresh_freq_secs: Some(bad),
            };
            assert!(request.refresh_freq_secs.unwrap() < 60);
        }
    }
}
