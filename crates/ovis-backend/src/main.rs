//! `ovis-backend` — the API server.

use std::net::SocketAddr;

use ovis_backend::config::ServerConfig;
use ovis_backend::{app, build_state, init_tracing, serve_with_shutdown, spawn_background_tasks};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse a bare `--config <path>` before anything else, so a bad path fails
    // before the logger even matters.
    let config_path = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0] == "--config")
        .map(|pair| pair[1].clone());

    let cfg = match ServerConfig::load(config_path.as_deref()) {
        Ok(cfg) => cfg,
        Err(err) => {
            // The logger is not up yet, and a configuration failure must be
            // visible regardless.
            eprintln!("ovis-backend: {err}");
            std::process::exit(2);
        }
    };

    init_tracing(cfg.json_logs());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting OVIS backend");
    tracing::info!("{}", cfg.summary());
    for warning in cfg.warnings() {
        tracing::warn!("{warning}");
    }

    let addr: SocketAddr = cfg.bind_address().parse()?;
    let grace = cfg.shutdown_grace();

    let state = build_state(cfg).await?;
    spawn_background_tasks(state.clone());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    serve_with_shutdown(listener, app(state), grace).await?;
    tracing::info!("stopped");

    Ok(())
}
