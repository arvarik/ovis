//! The OVIS backend: the single data plane behind both the UI and the CLI.
//!
//! Only this process holds credentials. The CLI speaks this HTTP API and needs no
//! database access of its own.

pub mod assets;
pub mod config;
pub mod error;
pub mod extract;
pub mod middleware;
pub mod routes;
pub mod services;
pub mod state;

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use axum::http::{HeaderName, Method};
use axum::Router;
use ovis_core::search::{EmbedClient, OsClient};
use ovis_core::CoreResult;
use sqlx::PgPool;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::config::ServerConfig;
use crate::middleware::{
    propagate_request_id, record_request, render_errors, require_bearer, MakeRequestUlid,
};
use crate::state::{AppState, BuildInfo, Caches, RuntimeMeta};

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Assemble the whole application: `/api/v1`, the embedded UI, and the
/// middleware stack.
///
/// Order, outermost first:
///
/// 1. request id — set and propagated, so every log line and error envelope can
///    be correlated
/// 2. tracing — one span per request carrying that id
/// 3. CORS
/// 4. bearer auth (only when `OVIS_API_TOKEN` is set)
/// 5. body limit
/// 6. compression — JSON with chunk text compresses 5-10×
///
/// The request timeout is applied per-route rather than globally: SSE streams are
/// long-lived by design and a global timeout would cut them off mid-flight.
pub fn app(state: AppState) -> Router {
    let cfg = state.cfg.clone();

    let cors = match cfg.cors_origin_list() {
        None => CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any),
        Some(origins) => {
            let parsed: Vec<axum::http::HeaderValue> =
                origins.iter().filter_map(|o| o.parse().ok()).collect();
            CorsLayer::new()
                .allow_origin(parsed)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PATCH,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers(Any)
        }
    };

    // Streaming routes carry no timeout; everything else does.
    let streaming = Router::new()
        .route("/pages/stream", axum::routing::get(routes::stream::stream))
        .layer(axum::middleware::from_fn(record_request))
        .with_state(state.clone());

    let timed = routes::api_router(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::enforce_timeout,
        ))
        // Inside the router so `MatchedPath` is populated: the metric label must
        // be the route template, not a raw path containing a document URL.
        .layer(axum::middleware::from_fn(record_request));

    let api = streaming.merge(timed);

    Router::new()
        .nest("/api/v1", api)
        .fallback(assets::static_handler)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(
                    REQUEST_ID_HEADER,
                    MakeRequestUlid::new(),
                ))
                .layer(PropagateRequestIdLayer::new(REQUEST_ID_HEADER))
                .layer(axum::middleware::from_fn(propagate_request_id))
                // Inside the request-id layer, outside everything that can fail,
                // so every error envelope gets the real id.
                .layer(axum::middleware::from_fn(render_errors))
                .layer(TraceLayer::new_for_http().make_span_with(
                    |request: &axum::http::Request<_>| {
                        let req_id = request
                            .headers()
                            .get("x-request-id")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("-");
                        tracing::info_span!(
                            "http",
                            method = %request.method(),
                            path = %request.uri().path(),
                            req_id = %req_id,
                        )
                    },
                ))
                .layer(cors)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    require_bearer,
                ))
                .layer(RequestBodyLimitLayer::new(cfg.body_limit_bytes))
                .layer(CompressionLayer::new().gzip(true).br(true)),
        )
}

/// Build the shared state: pool, clients, caches, runtime metadata, metrics.
pub async fn build_state(cfg: ServerConfig) -> anyhow::Result<AppState> {
    let cfg = Arc::new(cfg);

    let db: PgPool =
        ovis_core::db::create_pg_pool(&cfg.database_url, cfg.db_max_connections).await?;

    let os = OsClient::new(
        &cfg.opensearch_url,
        cfg.opensearch_username.as_deref(),
        cfg.opensearch_password.as_deref(),
    )?;

    let onyx = if cfg.onyx_configured() {
        Some(ovis_core::onyx::OnyxClient::new(
            cfg.onyx_api_url.as_deref().unwrap_or_default(),
            cfg.onyx_api_key.as_deref().unwrap_or_default(),
        )?)
    } else {
        None
    };

    let embed = match cfg.embed_api_url.as_deref() {
        Some(url) if !url.trim().is_empty() => Some(EmbedClient::new(url, &cfg.embed_model)?),
        _ => None,
    };

    // The runtime probe must succeed at least once: without it we do not know
    // which OpenSearch index to talk to, and guessing is exactly the defect this
    // replaces.
    let runtime = RuntimeMeta::load(&db, &os).await?;
    tracing::info!(
        index = %runtime.index_name,
        model = %runtime.embedding_model,
        dim = runtime.embedding_dim,
        schema_ok = runtime.schema.is_ok(),
        knn_ready = runtime.capabilities.knn_ready(),
        "resolved runtime metadata from search_settings"
    );
    if !runtime.capabilities.knn_ready() {
        tracing::warn!(
            "the live index has no populated knn_vector field, so mode=semantic and \
             mode=hybrid will degrade to keyword search (reported as \
             degraded=\"no_knn_field\")"
        );
    }

    let pending_deletes_enabled = ovis_core::db::pending_deletes::ensure_table(&db).await;
    let prune_enabled = ovis_core::db::prune::ensure_tables(&db).await;
    let trash_enabled = ovis_core::db::trash::ensure_tables(&db).await;
    let llm_enabled = ovis_core::db::llm::ensure_tables(&db).await;
    if prune_enabled && !trash_enabled {
        tracing::error!(
            "pruning is enabled but ovis.trash_document could not be created; the reaper will \
             refuse to delete. Deleting without a snapshot would be irreversible, which is \
             exactly what the trash exists to prevent."
        );
    }

    let metrics = install_metrics_recorder();

    Ok(AppState {
        db,
        os,
        onyx,
        embed,
        caches: Caches::new(),
        runtime: Arc::new(ArcSwap::from_pointee(runtime)),
        cfg,
        build: BuildInfo::current(),
        pending_deletes_enabled,
        prune: crate::state::PruneHandle::new(prune_enabled, trash_enabled),
        llm_enabled,
        metrics,
    })
}

/// Install the Prometheus recorder. Returns `None` if a recorder is already
/// installed (which happens in tests that build several states), in which case
/// `/system/metrics` reports itself unavailable rather than panicking.
fn install_metrics_recorder() -> Option<Arc<metrics_exporter_prometheus::PrometheusHandle>> {
    match metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder() {
        Ok(handle) => Some(Arc::new(handle)),
        Err(err) => {
            tracing::debug!(error = %err, "metrics recorder not installed");
            None
        }
    }
}

/// Start the background tasks: runtime refresh, pool heartbeat, pending-delete
/// drain. Each is independent, and each logs rather than propagating failure —
/// a transient blip must not take the server down.
pub fn spawn_background_tasks(state: AppState) {
    spawn_runtime_refresh(state.clone());
    spawn_pool_heartbeat(state.clone());
    if state.pending_deletes_enabled {
        spawn_pending_delete_drain(state.clone());
    }
    if state.prune.enabled {
        crate::services::prune_scan::spawn_scan_runner(state.clone());
        crate::services::prune_reaper::spawn_reaper(state);
    }
}

fn spawn_runtime_refresh(state: AppState) {
    let interval = Duration::from_secs(state.cfg.runtime_refresh_secs.max(5));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately and would redo the startup load.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match RuntimeMeta::load(&state.db, &state.os).await {
                Ok(fresh) => {
                    let previous = state.runtime();
                    if previous.index_name != fresh.index_name {
                        tracing::warn!(
                            from = %previous.index_name,
                            to = %fresh.index_name,
                            "the OpenSearch index changed; retargeting. This is what an Onyx \
                             re-embed switchover looks like."
                        );
                    }
                    if previous.schema.is_ok() && !fresh.schema.is_ok() {
                        tracing::error!(
                            missing = ?fresh.schema.missing_columns,
                            "the Onyx schema changed underneath us"
                        );
                    }
                    // Only on transition: an unchanged condition logged every
                    // minute buries the lines that matter.
                    if previous.capabilities.knn_ready() != fresh.capabilities.knn_ready() {
                        if fresh.capabilities.knn_ready() {
                            tracing::info!(
                                field = ?fresh.capabilities.knn_field,
                                "the index now has a populated knn_vector field; \
                                 semantic and hybrid search are live"
                            );
                        } else {
                            tracing::warn!(
                                "the index no longer has a populated knn_vector field; \
                                 semantic and hybrid search now degrade to keyword search"
                            );
                        }
                    }
                    state.runtime.store(Arc::new(fresh));
                }
                Err(err) => {
                    // Keep serving with the previous metadata; it was valid a
                    // minute ago and is far better than nothing.
                    tracing::warn!(error = %err, "runtime metadata refresh failed; keeping the previous value");
                }
            }
        }
    });
}

fn spawn_pool_heartbeat(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut was_healthy = true;
        loop {
            ticker.tick().await;
            let healthy = ovis_core::db::pool::ping(&state.db).await.is_ok();
            // Log transitions only: a per-30s "still fine" line is noise.
            if healthy != was_healthy {
                if healthy {
                    tracing::info!("postgres is reachable again");
                } else {
                    tracing::error!("postgres is unreachable; /system/health is now 503");
                }
                was_healthy = healthy;
            }
            metrics::gauge!("ovis_pg_up").set(if healthy { 1.0 } else { 0.0 });
        }
    });
}

fn spawn_pending_delete_drain(state: AppState) {
    let interval = Duration::from_secs(state.cfg.pending_delete_drain_secs.max(10));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let index = state.index_name();
            match ovis_core::db::pending_deletes::drain(&state.db, &state.os, &index, 100).await {
                Ok(report) if report.attempted > 0 => {
                    tracing::info!(
                        attempted = report.attempted,
                        cleared = report.cleared,
                        still_failing = report.still_failing,
                        "drained pending index deletes"
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "pending index delete drain failed");
                }
            }
        }
    });
}

/// Serve until shutdown, then drain — with a bound on the draining.
///
/// `axum::serve(..).with_graceful_shutdown(f)` begins draining when `f` resolves
/// and then waits for in-flight requests *indefinitely*. That is the wrong shape
/// for this server: an SSE stream is in-flight by design and can last an hour, so
/// an unbounded wait means `docker stop` hangs until its own kill timeout.
///
/// So: start draining the moment the signal arrives, and stop waiting after
/// `grace`. A stream that has not finished by then is cut, and the log says so
/// rather than leaving it a mystery.
pub async fn serve_with_shutdown(
    listener: tokio::net::TcpListener,
    router: Router,
    grace: Duration,
) -> std::io::Result<()> {
    let (signalled, mut begin_drain) = tokio::sync::watch::channel(false);
    let mut deadline = signalled.subscribe();
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = signalled.send(true);
    });

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        let _ = begin_drain.changed().await;
    });

    tokio::select! {
        result = server => result,
        _ = async move {
            let _ = deadline.changed().await;
            tokio::time::sleep(grace).await;
        } => {
            tracing::warn!(
                grace_secs = grace.as_secs(),
                "the drain window elapsed with requests still in flight; exiting anyway"
            );
            Ok(())
        }
    }
}

/// Resolve when the process is asked to stop.
///
/// SIGTERM is what `docker stop` and systemd send, and honouring it is what makes
/// in-flight requests drain instead of being severed.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install the Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install the SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl+C; draining in-flight requests"),
        _ = terminate => tracing::info!("received SIGTERM; draining in-flight requests"),
    }
}

/// Initialise tracing. Idempotent enough for tests, which may call it twice.
pub fn init_tracing(json: bool) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "ovis_backend=info,ovis_core=info,tower_http=warn,sqlx=warn".into());

    let registry = tracing_subscriber::registry().with(filter);
    let result = if json {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()
    } else {
        registry.with(tracing_subscriber::fmt::layer()).try_init()
    };
    if result.is_err() {
        tracing::debug!("tracing was already initialised");
    }
}

/// Load runtime metadata once, for callers that need it outside a running server.
pub async fn load_runtime(db: &PgPool, os: &OsClient) -> CoreResult<RuntimeMeta> {
    RuntimeMeta::load(db, os).await
}
