//! `GET /api/v1/pages/stream` — Server-Sent Events.
//!
//! The old stream materialised the entire result set before emitting anything,
//! then issued one OpenSearch call per row. This one pages Postgres by keyset in
//! batches of 200 and emits each row as it arrives, so the first byte is out
//! after one query.
//!
//! Contract:
//!
//! * `event: page` — one item, with an incrementing `id:` field so a client can
//!   tell how far it got.
//! * `:ka` comment every 15 s, so proxies do not reap an idle stream.
//! * `event: done` — `{"total_matched": n, "time_ms": t}`, always last on
//!   success.
//! * `event: error` — the same envelope body as an HTTP error, including
//!   `req_id`. A stream that dies mid-flight says so instead of just stopping.
//! * A client disconnect cancels the in-flight database work rather than
//!   discovering it on the next send.

use std::convert::Infallible;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use ovis_core::cursor::Cursor;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;

use crate::error::{AppError, RequestId};
use crate::extract::Query;
use crate::routes::pages::ListQuery;
use crate::services::pages as service;
use crate::state::AppState;

const KEEPALIVE_SECS: u64 = 15;

pub async fn stream(
    State(state): State<AppState>,
    extensions: axum::http::Extensions,
    Query(query): Query<ListQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Response> {
    let req_id = extensions
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| "-".to_string());

    // Validate before opening the stream: a bad parameter should be an HTTP 400,
    // not a 200 whose first event is an error.
    let prepared = (|| -> Result<_, AppError> {
        let limit = service::clamp_limit(query.limit, 1000, state.cfg.max_stream_limit);
        Ok((query.filter()?, query.sort_order()?, limit))
    })();

    let (filter, sort, limit) = match prepared {
        Ok(parts) => parts,
        Err(err) => return Err(err.into_response_with_id(&req_id)),
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        let started = Instant::now();
        let mut emitted: u64 = 0;
        let mut cursor: Option<Cursor> = None;

        // Resolving the plan is one bounded query; do it once for the stream.
        let plan = match ovis_core::db::documents::plan_connector_filter(&state.db, &filter).await {
            Ok(plan) => plan,
            Err(err) => {
                send_error(&tx, AppError::from(err), &req_id).await;
                return;
            }
        };

        loop {
            let remaining = limit.saturating_sub(emitted as i64);
            if remaining <= 0 {
                break;
            }
            let batch_size = remaining.min(service::STREAM_BATCH);

            // Cancel the query the moment the client goes away, rather than
            // finishing it and discovering the closed channel afterwards.
            let batch = tokio::select! {
                biased;
                _ = tx.closed() => {
                    tracing::debug!(req_id, emitted, "client disconnected; cancelling the stream");
                    return;
                }
                result = service::stream_batch(
                    &state, &filter, &plan, sort, cursor.as_ref(), batch_size,
                ) => result,
            };

            let items = match batch {
                Ok(items) => items,
                Err(err) => {
                    send_error(&tx, err, &req_id).await;
                    return;
                }
            };
            if items.is_empty() {
                break;
            }

            let exhausted = (items.len() as i64) < batch_size;
            cursor = items.last().map(|item| Cursor::after(sort, item));

            for item in items {
                let event = match Event::default()
                    .event("page")
                    .id(emitted.to_string())
                    .json_data(&item)
                {
                    Ok(event) => event,
                    Err(err) => {
                        tracing::error!(req_id, error = %err, "failed to serialise a page event");
                        continue;
                    }
                };
                if tx.send(Ok(event)).await.is_err() {
                    tracing::debug!(req_id, emitted, "client disconnected mid-batch");
                    return;
                }
                emitted += 1;
            }

            if exhausted {
                break;
            }
        }

        let payload = serde_json::json!({
            "total_matched": emitted,
            "time_ms": (started.elapsed().as_secs_f64() * 10_000.0).round() / 10.0,
        });
        if let Ok(event) = Event::default().event("done").json_data(payload) {
            let _ = tx.send(Ok(event)).await;
        }
        tracing::debug!(
            req_id,
            emitted,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "stream completed"
        );
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(KEEPALIVE_SECS))
            .text("ka"),
    ))
}

async fn send_error(
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
    err: AppError,
    req_id: &str,
) {
    if err.is_server_side() {
        tracing::error!(req_id, code = err.code(), detail = %err.log_detail(), "stream failed");
    }
    if let Ok(event) = Event::default()
        .event("error")
        .json_data(err.as_event_payload(req_id))
    {
        let _ = tx.send(Ok(event)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_stays_under_common_proxy_idle_timeouts() {
        // Caddy and nginx default to 60 s of idle before reaping a connection, so
        // the heartbeat has to be comfortably inside that.
        assert_eq!(KEEPALIVE_SECS, 15);
    }

    // The emitted wire bytes are asserted end-to-end in
    // `tests/api_contract.rs::sse_stream_emits_the_documented_contract`; these
    // cover the payloads those events carry.

    #[test]
    fn page_and_done_events_are_constructible_with_their_documented_fields() {
        assert!(Event::default()
            .event("page")
            .id("7")
            .json_data(serde_json::json!({ "id": "https://x/y" }))
            .is_ok());
        assert!(Event::default()
            .event("done")
            .json_data(serde_json::json!({ "total_matched": 431u64, "time_ms": 12.3 }))
            .is_ok());
    }

    #[test]
    fn the_error_event_carries_the_same_envelope_as_an_http_error() {
        let err = AppError::Database("connection refused".into());
        let payload = err.as_event_payload("01JREQ");
        assert_eq!(payload["code"], "DATABASE");
        assert_eq!(payload["status"], 500);
        assert_eq!(payload["req_id"], "01JREQ");
        // The driver detail must not travel with it.
        assert_eq!(payload["message"], "database error");
        assert!(!payload.to_string().contains("connection refused"));
        assert!(Event::default().event("error").json_data(&payload).is_ok());
    }

    #[test]
    fn batch_sizing_never_overshoots_the_requested_limit() {
        // The final batch must be trimmed, or a limit of 250 would emit 400 rows.
        let limit: i64 = 250;
        let mut emitted: i64 = 0;
        let mut batches = Vec::new();
        while emitted < limit {
            let batch = (limit - emitted).min(service::STREAM_BATCH);
            batches.push(batch);
            emitted += batch;
        }
        assert_eq!(batches, vec![200, 50]);
        assert_eq!(emitted, limit);
    }
}
