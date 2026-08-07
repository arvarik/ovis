//! Trash review: list, inspect, restore, hold, purge.
//!
//! Restore is a safe direction and behaves like one — no typed count, no
//! ceremony beyond the ordinary confirm-count drift check. Purge is the
//! opposite: it is the only genuinely irreversible operation in the whole
//! pruning system, so it demands the typed count at *every* size, not just
//! above the bulk threshold. There is deliberately no "empty trash" verb.

use ovis_core::api_types::ListResponse;
use ovis_core::db::prune as db;
use ovis_core::db::trash::{self, TrashFilter, TrashItem};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::AppError;
use crate::routes::prune::TrashQuery;
use crate::services::prune::{actor, guard};
use crate::state::AppState;

fn filter_from(query: &TrashQuery) -> TrashFilter {
    TrashFilter {
        connector_id: query.connector_id,
        document_id: query.document_id.clone(),
        hold: query.hold,
        expiring_within_days: query.expiring_within_days,
    }
}

pub async fn list(
    state: &AppState,
    query: TrashQuery,
) -> Result<ListResponse<TrashItem>, AppError> {
    guard(state)?;
    let limit = crate::services::pages::clamp_limit(query.limit, 50, state.cfg.max_page_size);
    let page = query.page.unwrap_or(1).max(1);
    let offset = (page - 1) * limit;
    let (items, total) = trash::list(&state.db, &filter_from(&query), limit, offset).await?;
    Ok(ListResponse {
        items,
        total,
        total_exact: true,
        page: Some(page),
        limit,
        next_cursor: None,
        has_more: offset + limit < total,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct TrashDetail {
    #[serde(flatten)]
    pub item: TrashItem,
    /// Reconstructed text from the snapshot's chunks — what the document said
    /// when it was deleted, readable without restoring it first.
    pub text: String,
    pub chunk_previews: Vec<String>,
    pub document: serde_json::Value,
    pub tags: Vec<serde_json::Value>,
}

pub async fn detail(state: &AppState, document_id: &str) -> Result<TrashDetail, AppError> {
    guard(state)?;
    let (items, _) = trash::list(
        &state.db,
        &TrashFilter {
            document_id: Some(document_id.to_string()),
            ..Default::default()
        },
        1,
        0,
    )
    .await?;
    let item = items.into_iter().next().ok_or_else(|| AppError::NotFound {
        what: "trashed document",
        id: document_id.to_string(),
    })?;

    let snapshot = trash::get_snapshot(&state.db, document_id)
        .await?
        .ok_or_else(|| AppError::NotFound {
            what: "trash snapshot",
            id: document_id.to_string(),
        })?;

    let contents: Vec<String> = snapshot
        .chunks
        .iter()
        .filter_map(|c| c.get("_source")?.get("content")?.as_str().map(String::from))
        .collect();
    let chunk_previews = contents
        .iter()
        .map(|c| c.chars().take(240).collect::<String>())
        .collect();

    Ok(TrashDetail {
        item,
        text: contents.join("\n\n"),
        chunk_previews,
        document: snapshot.document,
        tags: snapshot.tags,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrashBulkRequest {
    pub document_ids: Option<Vec<String>>,
    pub filter: Option<TrashFilterBody>,
    pub confirm_count: Option<i64>,
    /// Restore over a document the crawler brought back.
    #[serde(default)]
    pub overwrite: bool,
    /// Purge only: the count typed back by the operator.
    pub typed_count: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrashFilterBody {
    pub connector_id: Option<i32>,
    pub hold: Option<bool>,
    pub expiring_within_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrashBulkResponse {
    pub success: bool,
    pub requested: i64,
    pub changed: i64,
    pub failed: Vec<TrashFailure>,
    pub action: String,
    /// Restore only: per-document detail, because a restore that silently
    /// dropped a document's tags would be worse than one that says so.
    pub outcomes: Vec<trash::RestoreOutcome>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrashFailure {
    pub document_id: String,
    pub code: String,
    pub message: String,
}

/// Resolve a selection and enforce the drift check, exactly as the candidate
/// bulk endpoints do.
async fn resolve(
    state: &AppState,
    op: &str,
    request: &TrashBulkRequest,
) -> Result<Vec<String>, AppError> {
    let ids = match (&request.document_ids, &request.filter) {
        (Some(ids), None) => {
            if ids.is_empty() {
                return Err(AppError::BadRequest(
                    "document_ids must not be empty".into(),
                ));
            }
            ids.clone()
        }
        (None, Some(body)) => {
            trash::ids_matching(
                &state.db,
                &TrashFilter {
                    connector_id: body.connector_id,
                    document_id: None,
                    hold: body.hold,
                    expiring_within_days: body.expiring_within_days,
                },
            )
            .await?
        }
        (Some(_), Some(_)) => {
            return Err(AppError::BadRequest(
                "pass either document_ids or filter, not both".into(),
            ))
        }
        (None, None) => {
            return Err(AppError::BadRequest(
                "pass document_ids or a filter; an empty selector would select nothing".into(),
            ))
        }
    };

    if let Some(confirmed) = request.confirm_count {
        if confirmed != ids.len() as i64 {
            return Err(AppError::Conflict(format!(
                "the selection matches {} snapshots, not the confirmed {confirmed}; nothing was \
                 changed. Re-check and resend with confirm_count={}",
                ids.len(),
                ids.len()
            )));
        }
    }
    if ids.is_empty() {
        return Err(AppError::BadRequest(format!(
            "{op}: the selection matches no snapshots"
        )));
    }
    Ok(ids)
}

pub async fn restore(
    state: &AppState,
    request: TrashBulkRequest,
) -> Result<TrashBulkResponse, AppError> {
    guard(state)?;
    let ids = resolve(state, "restore", &request).await?;
    let who = actor(state);
    let index = state.index_name();

    let mut changed = 0i64;
    let mut failed = Vec::new();
    let mut outcomes = Vec::new();

    for document_id in &ids {
        match trash::restore(&state.db, &state.os, &index, document_id, request.overwrite).await {
            Ok(outcome) => {
                changed += 1;
                db::audit(
                    &state.db,
                    who,
                    "trash_restored",
                    Some(document_id),
                    None,
                    None,
                    Some(json!({
                        "chunks_restored": outcome.chunks_restored,
                        "tags_restored": outcome.tags_restored,
                        "cc_pairs_restored": outcome.cc_pairs_restored,
                        "skipped_tags": outcome.skipped_tags,
                        "skipped_cc_pairs": outcome.skipped_cc_pairs,
                        "index_restore_pending": outcome.index_restore_pending,
                        "overwrite": request.overwrite,
                    })),
                )
                .await;
                outcomes.push(outcome);
            }
            Err(err) => failed.push(TrashFailure {
                document_id: document_id.clone(),
                code: err.code().to_string(),
                message: err.to_string(),
            }),
        }
    }

    if changed > 0 {
        state.caches.invalidate_document_scoped().await;
    }

    Ok(TrashBulkResponse {
        success: failed.is_empty(),
        requested: ids.len() as i64,
        changed,
        failed,
        action: "restored".into(),
        outcomes,
    })
}

/// Permanently drop snapshots.
///
/// The only irreversible verb in the system, and the only one that requires
/// the typed count regardless of batch size. There is no "empty trash":
/// removing everything has to be spelled out as a selection and a number.
pub async fn purge(
    state: &AppState,
    request: TrashBulkRequest,
) -> Result<TrashBulkResponse, AppError> {
    guard(state)?;
    let ids = resolve(state, "purge", &request).await?;

    match request.typed_count {
        Some(typed) if typed == ids.len() as i64 => {}
        Some(typed) => {
            return Err(AppError::Conflict(format!(
                "typed_count {typed} does not match the {} snapshots selected; nothing was purged",
                ids.len()
            )))
        }
        None => {
            return Err(AppError::BadRequest(format!(
                "purge permanently destroys {} snapshot(s) and cannot be undone; resend with \
                 typed_count={} to confirm",
                ids.len(),
                ids.len()
            )))
        }
    }

    // A held snapshot is pinned deliberately; bulk purge must not sweep it up.
    let held: Vec<String> = trash::ids_matching(
        &state.db,
        &TrashFilter {
            hold: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let (purgeable, skipped): (Vec<String>, Vec<String>) =
        ids.iter().cloned().partition(|id| !held.contains(id));

    let purged = trash::purge(&state.db, &purgeable).await?;
    let who = actor(state);
    db::audit(
        &state.db,
        who,
        "trash_purged",
        None,
        None,
        None,
        Some(json!({
            "count": purged,
            "requested": ids.len(),
            "skipped_held": skipped.len(),
            "reason": "manual",
        })),
    )
    .await;

    Ok(TrashBulkResponse {
        success: skipped.is_empty(),
        requested: ids.len() as i64,
        changed: purged as i64,
        failed: skipped
            .into_iter()
            .map(|document_id| TrashFailure {
                document_id,
                code: "ON_HOLD".into(),
                message: "the snapshot is on hold; release the hold before purging".into(),
            })
            .collect(),
        action: "purged".into(),
        outcomes: Vec::new(),
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrashHoldRequest {
    pub document_ids: Vec<String>,
    pub hold: bool,
}

pub async fn set_hold(
    state: &AppState,
    request: TrashHoldRequest,
) -> Result<TrashBulkResponse, AppError> {
    guard(state)?;
    if request.document_ids.is_empty() {
        return Err(AppError::BadRequest(
            "document_ids must not be empty".into(),
        ));
    }
    let mut changed = 0i64;
    let mut failed = Vec::new();
    for document_id in &request.document_ids {
        match trash::set_hold(&state.db, document_id, request.hold).await {
            Ok(true) => changed += 1,
            Ok(false) => failed.push(TrashFailure {
                document_id: document_id.clone(),
                code: "NOT_FOUND".into(),
                message: "no un-restored snapshot with that id".into(),
            }),
            Err(err) => failed.push(TrashFailure {
                document_id: document_id.clone(),
                code: "ERROR".into(),
                message: err.to_string(),
            }),
        }
    }
    db::audit(
        &state.db,
        actor(state),
        if request.hold {
            "trash_held"
        } else {
            "trash_hold_released"
        },
        None,
        None,
        None,
        Some(json!({ "count": changed })),
    )
    .await;
    Ok(TrashBulkResponse {
        success: failed.is_empty(),
        requested: request.document_ids.len() as i64,
        changed,
        failed,
        action: if request.hold { "held" } else { "released" }.into(),
        outcomes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    /// Purge is the only irreversible verb, and the one place a typed count is
    /// mandatory at any size. Asserted structurally so the requirement cannot
    /// be dropped without the test noticing.
    #[test]
    fn purge_always_requires_a_typed_count() {
        let source = include_str!("trash.rs");
        let body = source
            .split("pub async fn purge(")
            .nth(1)
            .expect("purge exists");
        let body = &body[..body.find("pub async fn").unwrap_or(body.len())];
        assert!(
            body.contains("typed_count"),
            "purge must check the typed count"
        );
        assert!(
            body.contains("cannot be undone"),
            "the refusal must say why it is asking"
        );
    }

    /// Restore is a safe direction: it must not demand a typed count.
    #[test]
    fn restore_does_not_demand_a_typed_count() {
        let source = include_str!("trash.rs");
        let body = source
            .split("pub async fn restore(")
            .nth(1)
            .expect("restore exists");
        let body = &body[..body.find("pub async fn").unwrap_or(body.len())];
        assert!(
            !body.contains("typed_count"),
            "putting documents back must stay cheap"
        );
    }

    /// There is no bulk "delete everything" verb. Emptying the trash has to be
    /// expressed as a selection and a number, so it cannot be a single click.
    #[test]
    fn no_empty_trash_verb_exists() {
        let source = include_str!("trash.rs");
        // Assembled at runtime so this test cannot match its own source.
        for name in ["empty", "purge_all", "clear_trash", "empty_trash"] {
            let forbidden = format!("fn {name}");
            assert!(
                !source.contains(&forbidden),
                "`{forbidden}` must not exist: emptying the trash has to be expressed as a \
                 selection and a typed count, never a single verb"
            );
        }
    }
}
