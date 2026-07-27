use anyhow::Result;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, Color, Table};

use crate::models::{ChunkRecord, ConnectorSummary, DocumentRecord};
use ovis_prune::{DocumentWithContent, PruneConfig, PruningEngine};

use serde_json::json;
use std::io::{self, Write};

use crate::cli::{Cli, OutputFormat};
use crate::formatters::Formatter;
use crate::tui::run_tui;

/// Generate standard sample documents for standalone or offline CLI operation.
pub fn get_sample_documents() -> Vec<DocumentRecord> {
    vec![
        DocumentRecord {
            id: "https://docs.onyx.app/web".to_string(),
            from_beginning: Some(true),
            semantic_id: "Web Connector Docs".to_string(),
            link: Some("https://docs.onyx.app/web".to_string()),
            doc_updated_at: Some(chrono::Utc::now() - chrono::Duration::hours(24)),
            primary_owners: Some(vec!["docs-team@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "web", "connector_id": 2, "chunks": 4}),
        },
        DocumentRecord {
            id: "https://github.com/onyx-dot-app".to_string(),
            from_beginning: Some(true),
            semantic_id: "Onyx GitHub Repository".to_string(),
            link: Some("https://github.com/onyx-dot-app".to_string()),
            doc_updated_at: Some(chrono::Utc::now() - chrono::Duration::hours(48)),
            primary_owners: Some(vec!["devs@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "github", "connector_id": 1, "chunks": 12}),
        },
        DocumentRecord {
            id: "https://docs.onyx.app/setup".to_string(),
            from_beginning: Some(true),
            semantic_id: "Onyx Setup Guide".to_string(),
            link: Some("https://docs.onyx.app/setup".to_string()),
            doc_updated_at: Some(chrono::Utc::now() - chrono::Duration::hours(12)),
            primary_owners: Some(vec!["devops@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "web", "connector_id": 2, "chunks": 6}),
        },
        DocumentRecord {
            id: "https://docs.onyx.app/api".to_string(),
            from_beginning: Some(true),
            semantic_id: "Onyx REST API Reference".to_string(),
            link: Some("https://docs.onyx.app/api".to_string()),
            doc_updated_at: Some(chrono::Utc::now() - chrono::Duration::hours(6)),
            primary_owners: Some(vec!["api-team@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "web", "connector_id": 2, "chunks": 8}),
        },
        DocumentRecord {
            id: "s3://company-docs/architecture.pdf".to_string(),
            from_beginning: Some(true),
            semantic_id: "Onyx Architecture Overview".to_string(),
            link: Some("s3://company-docs/architecture.pdf".to_string()),
            doc_updated_at: Some(chrono::Utc::now() - chrono::Duration::hours(72)),
            primary_owners: Some(vec!["arch-team@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "file", "connector_id": 3, "chunks": 15}),
        },
    ]
}

/// Generate sample vector chunks corresponding to the sample documents.
pub fn get_sample_chunks() -> Vec<ChunkRecord> {
    vec![
        ChunkRecord {
            chunk_id: 0,
            document_id: "https://docs.onyx.app/web".to_string(),
            content: "The web connector allows Onyx to crawl and index HTTP/HTTPS websites using customizable depth limits and rate throttling.".to_string(),
            title: Some("Web Connector Setup".to_string()),
            source_type: "web".to_string(),
            metadata: json!({"connector_id": 2}),
            embeddings: None,
        },
        ChunkRecord {
            chunk_id: 1,
            document_id: "https://docs.onyx.app/web".to_string(),
            content: "Configuration options include base_url, max_depth, allow_domains, and authentication headers.".to_string(),
            title: Some("Web Connector Parameters".to_string()),
            source_type: "web".to_string(),
            metadata: json!({"connector_id": 2}),
            embeddings: None,
        },
        ChunkRecord {
            chunk_id: 0,
            document_id: "https://github.com/onyx-dot-app".to_string(),
            content: "Onyx is an open-source AI assistant and enterprise search platform that connects to all your tools and documentation.".to_string(),
            title: Some("Onyx GitHub Overview".to_string()),
            source_type: "github".to_string(),
            metadata: json!({"connector_id": 1}),
            embeddings: None,
        },
        ChunkRecord {
            chunk_id: 0,
            document_id: "https://docs.onyx.app/setup".to_string(),
            content: "To set up Onyx via Docker Compose: run `docker compose up -d` to launch PostgreSQL, OpenSearch, Vespa, and the API server.".to_string(),
            title: Some("Quickstart Docker Guide".to_string()),
            source_type: "web".to_string(),
            metadata: json!({"connector_id": 2}),
            embeddings: None,
        },
        ChunkRecord {
            chunk_id: 0,
            document_id: "https://docs.onyx.app/api".to_string(),
            content: "The OVIS REST API provides endpoints `/api/document`, `/api/connector`, and `/api/search` for programmatic management.".to_string(),
            title: Some("API Endpoint Overview".to_string()),
            source_type: "web".to_string(),
            metadata: json!({"connector_id": 2}),
            embeddings: None,
        },
        ChunkRecord {
            chunk_id: 0,
            document_id: "s3://company-docs/architecture.pdf".to_string(),
            content: "Architecture specification: OVIS uses a single-binary architecture with PostgreSQL metadata storage and OpenSearch index.".to_string(),
            title: Some("System Architecture".to_string()),
            source_type: "file".to_string(),
            metadata: json!({"connector_id": 3}),
            embeddings: None,
        },
    ]
}

/// Generate sample connector summaries for offline CLI operation.
pub fn get_sample_connectors() -> Vec<ConnectorSummary> {
    vec![
        ConnectorSummary {
            connector_id: 1,
            connector_name: "Onyx GitHub Repository".to_string(),
            connector_source: "github".to_string(),
            disabled: false,
            total_pages: 12,
            last_indexed_at: Some(chrono::Utc::now() - chrono::Duration::hours(48)),
        },
        ConnectorSummary {
            connector_id: 2,
            connector_name: "Onyx Web Documentation".to_string(),
            connector_source: "web".to_string(),
            disabled: false,
            total_pages: 4,
            last_indexed_at: Some(chrono::Utc::now() - chrono::Duration::hours(24)),
        },
        ConnectorSummary {
            connector_id: 3,
            connector_name: "Internal S3 Architecture Bucket".to_string(),
            connector_source: "file".to_string(),
            disabled: false,
            total_pages: 15,
            last_indexed_at: Some(chrono::Utc::now() - chrono::Duration::hours(72)),
        },
    ]
}

pub fn get_pid_file_path() -> std::path::PathBuf {
    let mut dir = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    dir.push(".ovis");
    let _ = std::fs::create_dir_all(&dir);
    dir.push("ovis-server.pid");
    dir
}

pub fn read_pid_file() -> Option<u32> {
    let pid_file = get_pid_file_path();
    if pid_file.exists() {
        if let Ok(content) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = content.trim().parse::<u32>() {
                return Some(pid);
            }
        }
    }
    None
}

pub fn write_pid_file(pid: u32) -> io::Result<()> {
    let pid_file = get_pid_file_path();
    std::fs::write(pid_file, pid.to_string())
}

pub fn remove_pid_file() {
    let pid_file = get_pid_file_path();
    let _ = std::fs::remove_file(pid_file);
}

/// Resolve connection endpoints from flags and the environment.
///
/// This used to fall back to a **hardcoded production DSN, password included**,
/// which was compiled into every binary and printed to stdout on startup. There
/// is now no compiled-in credential and no host probing: either the connection
/// details are supplied, or the command says what is missing. (The CLI redesign
/// removes the need for them entirely — it will speak the OVIS HTTP API, and only
/// the backend will hold credentials.)
pub fn resolve_endpoints(cli: &Cli) -> Result<(String, String)> {
    let db_url = cli
        .db_dsn
        .clone()
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .or_else(|| std::env::var("POSTGRES_URL").ok())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no database connection configured. Pass --db-dsn or set DATABASE_URL \
                 (point it at Postgres directly, not pgbouncer)."
            )
        })?;

    let search_url = cli
        .opensearch_url
        .clone()
        .or_else(|| std::env::var("OPENSEARCH_URL").ok())
        .or_else(|| std::env::var("SEARCH_ENGINE_URL").ok())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no OpenSearch endpoint configured. Pass --opensearch-url or set \
                 OPENSEARCH_URL."
            )
        })?;

    Ok((db_url, search_url))
}

/// Backwards-compatible shape for existing call sites: returns `None` when the
/// endpoints are not configured, rather than inventing them.
pub async fn auto_detect_onyx_stack(cli: &Cli, _force_auto: bool) -> Option<(String, String)> {
    match resolve_endpoints(cli) {
        Ok(pair) => Some(pair),
        Err(err) => {
            eprintln!("[ERROR] {err}");
            None
        }
    }
}

/// Handles starting the server in foreground or detached daemon mode.
pub async fn handle_server_start(
    cli: &Cli,
    port: u16,
    host: &str,
    detach: bool,
    auto_detect: bool,
) -> Result<()> {
    if detach {
        if let Some(existing_pid) = read_pid_file() {
            println!("[INFO] Server is already running in background (PID: {}).", existing_pid);
            println!("[INFO] Use 'ovis server status' or 'ovis server stop' to manage.");
            return Ok(());
        }

        let current_exe = std::env::current_exe()?;
        let mut cmd = std::process::Command::new(current_exe);
        cmd.arg("server")
            .arg("start")
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg(host);

        if auto_detect || cli.auto_detect {
            cmd.arg("--auto-detect");
        }
        if let Some(ref dsn) = cli.db_dsn {
            cmd.arg("--db-dsn").arg(dsn);
        }
        if let Some(ref search) = cli.opensearch_url {
            cmd.arg("--opensearch-url").arg(search);
        }

        let child = cmd.spawn()?;
        let pid = child.id();
        write_pid_file(pid)?;

        println!("[INFO] 🚀 OVIS Web & Frontend SPA started in background (PID: {}, Port: {})", pid, port);
        println!("[INFO] Open dashboard at http://{}:{}", if host == "0.0.0.0" { "localhost" } else { host }, port);
        return Ok(());
    }

    let (db_url, search_url) = resolve_endpoints(cli)?;
    write_pid_file(std::process::id())?;

    // Build the server through the same configuration path `ovis-backend` uses,
    // so the CLI-launched server is not a second, divergent bootstrap.
    let mut cfg = ovis_backend::config::ServerConfig::default();
    cfg.host = host.to_string();
    cfg.port = port;
    cfg.database_url = db_url;
    cfg.opensearch_url = search_url;
    cfg.onyx_api_url = std::env::var("ONYX_API_URL").ok();
    cfg.onyx_api_key = std::env::var("ONYX_API_KEY").ok();
    cfg.embed_api_url = std::env::var("EMBED_API_URL").ok();
    cfg.api_token = std::env::var("OVIS_API_TOKEN").ok();

    ovis_backend::init_tracing(false);
    // Redacted: never print the DSN, which carries the password.
    println!("[INFO] 🌐 OVIS dashboard at http://{}:{}", host, port);
    println!("[INFO] 🔌 REST API at http://{}:{}/api/v1", host, port);
    println!("[INFO] {}", cfg.summary());
    println!("[INFO] Press Ctrl+C to terminate the server process.");

    let grace = cfg.shutdown_grace();
    let state = ovis_backend::build_state(cfg).await?;
    ovis_backend::spawn_background_tasks(state.clone());
    let router = ovis_backend::app(state);

    let addr: std::net::SocketAddr = format!("{}:{}", host, port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let result = ovis_backend::serve_with_shutdown(listener, router, grace).await;
    remove_pid_file();
    result?;

    Ok(())
}

/// Handles stopping the server process cleanly.
pub async fn handle_server_stop(port: u16) -> Result<()> {
    if let Some(pid) = read_pid_file() {
        println!("[INFO] 🛑 Stopping OVIS server background process (PID: {})...", pid);
        #[cfg(unix)]
        {
            let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill").args(&["/F", "/PID", &pid.to_string()]).output();
        }
        remove_pid_file();
        println!("[INFO] Server (PID: {}) stopped successfully.", pid);
    } else {
        println!("[WARN] No active OVIS server PID lockfile found.");
        println!("[INFO] Checking port {} via process table...", port);
        #[cfg(unix)]
        {
            if let Ok(output) = std::process::Command::new("lsof").args(&["-t", "-i", &format!(":{}", port)]).output() {
                let pid_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !pid_str.is_empty() {
                    let _ = std::process::Command::new("kill").arg(&pid_str).output();
                    println!("[INFO] Killed process listening on port {} (PID: {}).", port, pid_str);
                    return Ok(());
                }
            }
        }
        println!("[INFO] No running OVIS server process found on port {}.", port);
    }
    Ok(())
}

/// Handles restarting the server.
pub async fn handle_server_restart(
    cli: &Cli,
    port: u16,
    host: &str,
    detach: bool,
    auto_detect: bool,
) -> Result<()> {
    println!("[INFO] 🔄 Restarting OVIS Web & Frontend SPA server...");
    let _ = handle_server_stop(port).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    handle_server_start(cli, port, host, detach, auto_detect).await
}

/// Handles querying server status & health.
pub async fn handle_server_status(port: u16) -> Result<()> {
    let pid_opt = read_pid_file();
    let health_url = format!("http://127.0.0.1:{}/api/health", port);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;

    match client.get(&health_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            println!("=== OVIS Server Status ===");
            println!("Status:   🟢 RUNNING");
            if let Some(pid) = pid_opt {
                println!("PID:      {}", pid);
            }
            println!("Port:     {}", port);
            println!("Endpoint: http://localhost:{}", port);
            println!("Health:   OK (HTTP 200)");
        }
        _ => {
            println!("=== OVIS Server Status ===");
            println!("Status:   🔴 STOPPED / UNREACHABLE");
            if let Some(pid) = pid_opt {
                println!("PID:      {} (Stale lockfile)", pid);
            }
            println!("Port:     {}", port);
        }
    }

    Ok(())
}

/// Backward compatible legacy handle_server wrapper.
pub async fn handle_server(cli: &Cli, port: u16, host: &str) -> Result<()> {
    handle_server_start(cli, port, host, false, cli.auto_detect).await
}

/// Handles the `connector list` subcommand.
pub async fn handle_connector_list(cli: &Cli) -> Result<()> {
    let mut stdout = io::stdout();

    let connectors = if let Some(ref dsn) = cli.db_dsn {
        match crate::compat::create_pg_pool(dsn).await {
            Ok(pool) => match crate::compat::fetch_connector_summaries(&pool).await {
                Ok(conns) => conns,
                Err(e) => {
                    eprintln!("[WARN] Failed to fetch connectors from DB ({}). Using sample data.", e);
                    get_sample_connectors()
                }
            },
            Err(e) => {
                eprintln!("[WARN] Failed to connect to PostgreSQL ({}). Using sample data.", e);
                get_sample_connectors()
            }
        }
    } else {
        get_sample_connectors()
    };

    Formatter::print_connectors(&mut stdout, &connectors, cli.format)?;
    Ok(())
}

/// Handles the `page list` subcommand.
pub async fn handle_page_list(
    cli: &Cli,
    connector: Option<i32>,
    source: Option<String>,
    search: Option<String>,
    limit: usize,
    offset: usize,
) -> Result<()> {
    let mut stdout = io::stdout();

    let docs = if let Some(ref dsn) = cli.db_dsn {
        match crate::compat::create_pg_pool(dsn).await {
            Ok(pool) => match crate::compat::fetch_documents(
                &pool,
                connector,
                source.as_deref(),
                search.as_deref(),
                limit as i64,
                offset as i64,
            )
            .await
            {
                Ok(fetched) => fetched,
                Err(e) => {
                    eprintln!("[WARN] Failed to fetch documents from DB ({}). Using sample data.", e);
                    get_sample_documents()
                }
            },
            Err(e) => {
                eprintln!("[WARN] Failed to connect to PostgreSQL ({}). Using sample data.", e);
                get_sample_documents()
            }
        }
    } else {
        let mut docs = get_sample_documents();
        if let Some(cid) = connector {
            docs.retain(|d| {
                d.metadata
                    .get("connector_id")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32)
                    == Some(cid)
            });
        }
        if let Some(ref src) = source {
            let src_lower = src.to_lowercase();
            docs.retain(|d| {
                d.metadata
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase()
                    == src_lower
            });
        }
        if let Some(ref q) = search {
            let q_lower = q.to_lowercase();
            docs.retain(|d| {
                d.id.to_lowercase().contains(&q_lower)
                    || d.semantic_id.to_lowercase().contains(&q_lower)
            });
        }
        let start = offset.min(docs.len());
        let end = (start + limit).min(docs.len());
        docs[start..end].to_vec()
    };

    Formatter::print_documents(&mut stdout, &docs, cli.format)?;
    Ok(())
}

/// Handles the `page inspect` subcommand.
pub async fn handle_page_inspect(cli: &Cli, id: &str, raw: bool) -> Result<()> {
    let mut stdout = io::stdout();

    let (doc, chunks) = if let Some(ref dsn) = cli.db_dsn {
        let pool_res = crate::compat::create_pg_pool(dsn).await;
        if let Ok(pool) = pool_res {
            let docs = crate::compat::fetch_documents(&pool, None, None, Some(id), 1, 0).await?;
            if let Some(target_doc) = docs.into_iter().next() {
                let chunks = match cli.opensearch_url.as_deref() {
                    Some(search_url) => {
                        let index = crate::compat::resolve_index(&pool).await?;
                        let os = crate::compat::os_client(search_url)?;
                        crate::compat::get_document_chunks(&os, &index, &target_doc.id)
                            .await
                            .unwrap_or_default()
                    }
                    None => get_sample_chunks()
                        .into_iter()
                        .filter(|c| c.document_id == target_doc.id)
                        .collect(),
                };
                (target_doc, chunks)
            } else {
                return Err(anyhow::anyhow!("Document not found for ID: {}", id));
            }
        } else {
            get_sample_inspect_doc(id)
        }
    } else {
        get_sample_inspect_doc(id)
    };

    Formatter::print_document_inspection(&mut stdout, &doc, &chunks, raw, cli.format)?;
    Ok(())
}

fn get_sample_inspect_doc(id: &str) -> (DocumentRecord, Vec<ChunkRecord>) {
    let docs = get_sample_documents();
    let doc = docs
        .into_iter()
        .find(|d| d.id == id || d.semantic_id.to_lowercase().contains(&id.to_lowercase()))
        .unwrap_or_else(|| DocumentRecord {
            id: id.to_string(),
            from_beginning: Some(true),
            semantic_id: format!("Document {}", id),
            link: Some(id.to_string()),
            doc_updated_at: Some(chrono::Utc::now()),
            primary_owners: Some(vec!["owner@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "web", "chunks": 1}),
        });

    let chunks = get_sample_chunks()
        .into_iter()
        .filter(|c| c.document_id == doc.id)
        .collect();

    (doc, chunks)
}

/// Handles the `page edit` subcommand.
pub async fn handle_page_edit(
    cli: &Cli,
    id: &str,
    title: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<()> {
    let (mut doc, chunks) = if let Some(ref dsn) = cli.db_dsn {
        let pool_res = crate::compat::create_pg_pool(dsn).await;
        if let Ok(pool) = pool_res {
            let docs = crate::compat::fetch_documents(&pool, None, None, Some(id), 1, 0).await?;
            if let Some(target_doc) = docs.into_iter().next() {
                let chunks = match cli.opensearch_url.as_deref() {
                    Some(search_url) => {
                        let index = crate::compat::resolve_index(&pool).await?;
                        let os = crate::compat::os_client(search_url)?;
                        crate::compat::get_document_chunks(&os, &index, &target_doc.id)
                            .await
                            .unwrap_or_default()
                    }
                    None => get_sample_chunks()
                        .into_iter()
                        .filter(|c| c.document_id == target_doc.id)
                        .collect(),
                };
                (target_doc, chunks)
            } else {
                get_sample_inspect_doc(id)
            }
        } else {
            get_sample_inspect_doc(id)
        }
    } else {
        get_sample_inspect_doc(id)
    };

    if let Some(ref new_title) = title {
        doc.semantic_id = new_title.clone();
    }

    if let Some(ref new_tags) = tags {
        if let Some(obj) = doc.metadata.as_object_mut() {
            obj.insert("tags".to_string(), json!(new_tags));
        } else {
            doc.metadata = json!({ "tags": new_tags });
        }
    }

    // `doc_updated_at` is Onyx's crawl timestamp, not ours: `last_modified >
    // last_synced` is what drives Onyx's sync detection, and writing it corrupts
    // that. The old edit path set it on every save.

    if let Some(ref dsn) = cli.db_dsn {
        if let Ok(pool) = crate::compat::create_pg_pool(dsn).await {
            // Only the tags we were asked to set are merged, so no other
            // metadata key is lost.
            let metadata_merge = tags
                .as_ref()
                .map(|new_tags| json!({ "tags": new_tags }));
            crate::compat::update_document(
                &pool,
                &doc.id,
                title.as_deref(),
                metadata_merge.as_ref(),
            )
            .await?;

            if let (Some(search_url), Some(new_title)) =
                (cli.opensearch_url.as_deref(), title.as_deref())
            {
                let index = crate::compat::resolve_index(&pool).await?;
                let os = crate::compat::os_client(search_url)?;
                let updated =
                    crate::compat::update_index_title(&os, &index, &doc.id, new_title).await?;
                println!("[INFO] Synced the new title to {updated} indexed chunk(s).");
            }
        }
    }

    println!("[INFO] Updated document metadata:");
    let mut stdout = io::stdout();
    Formatter::print_document_inspection(&mut stdout, &doc, &chunks, false, cli.format)?;
    Ok(())
}

/// Handles the `page delete` subcommand.
pub async fn handle_page_delete(cli: &Cli, id: &str, yes: bool) -> Result<()> {
    let should_proceed = yes || cli.yes;

    if !should_proceed {
        print!("Are you sure you want to delete page '{}'? [y/N]: ", id);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Deletion cancelled.");
            return Ok(());
        }
    }

    // No database configured means nothing was deleted, and saying otherwise is
    // exactly the kind of fake success this redesign removes.
    let Some(ref dsn) = cli.db_dsn else {
        anyhow::bail!(
            "no database configured, so nothing was deleted. Pass --db-dsn or set DATABASE_URL."
        );
    };
    let Some(search_url) = cli.opensearch_url.as_deref() else {
        anyhow::bail!(
            "no OpenSearch endpoint configured; refusing to delete the Postgres row and leave \
             its chunks orphaned. Pass --opensearch-url or set OPENSEARCH_URL."
        );
    };

    let pool = crate::compat::create_pg_pool(dsn).await?;
    let index = crate::compat::resolve_index(&pool).await?;
    let os = crate::compat::os_client(search_url)?;
    let outcome = crate::compat::delete_document(&pool, &os, &index, id).await?;

    println!(
        "Deleted '{}': {} chunk(s) removed from the index.",
        id, outcome.chunks_deleted
    );
    if outcome.index_cleanup_pending {
        println!(
            "[WARN] The Postgres row is gone but the index cleanup failed; it is queued in \
             ovis.pending_index_deletes and the server retries it."
        );
    }
    if outcome.recrawl_risk {
        println!(
            "[WARN] This document's connector is still active, so the next scheduled refresh \
             will likely crawl it again."
        );
    }
    Ok(())
}

/// Handles the `prune run` subcommand.
pub async fn handle_prune_run(
    cli: &Cli,
    config_path: &str,
    dry_run: bool,
    force: bool,
) -> Result<()> {
    let mut config = PruneConfig::from_file(config_path).unwrap_or_else(|_| {
        eprintln!("[INFO] Config file '{}' not found or invalid. Using default PruneConfig.", config_path);
        PruneConfig::default()
    });

    config.execution.dry_run = dry_run || !force;

    let engine = PruningEngine::new(config)?;

    let (docs, live) = if let Some(ref dsn) = cli.db_dsn {
        if let Ok(pool) = crate::compat::create_pg_pool(dsn).await {
            let fetched_records = crate::compat::fetch_documents(&pool, None, None, None, 1000, 0)
                .await
                .unwrap_or_default();

            let search_url = cli.opensearch_url.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "prune needs an OpenSearch endpoint to read document content. Pass \
                     --opensearch-url or set OPENSEARCH_URL."
                )
            })?;
            let index = crate::compat::resolve_index(&pool).await?;
            let os = crate::compat::os_client(&search_url)?;

            let mut live_docs = Vec::new();
            for rec in fetched_records {
                let chunks = crate::compat::get_document_chunks(&os, &index, &rec.id)
                    .await
                    .unwrap_or_default();
                let full_content = if !chunks.is_empty() {
                    chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join("\n\n")
                } else {
                    rec.semantic_id.clone()
                };

                let connector_id = rec
                    .metadata
                    .get("connector_id")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32)
                    .unwrap_or(1);

                live_docs.push(DocumentWithContent {
                    id: rec.id.clone(),
                    semantic_id: rec.semantic_id.clone(),
                    connector_id,
                    link: rec.link.clone(),
                    content: full_content,
                    updated_at: rec.doc_updated_at,
                    metadata: rec.metadata.clone(),
                });
            }

            if !live_docs.is_empty() {
                (live_docs, Some((pool, os, index)))
            } else {
                (get_sample_prune_docs(), Some((pool, os, index)))
            }
        } else {
            (get_sample_prune_docs(), None)
        }
    } else {
        (get_sample_prune_docs(), None)
    };

    let report = engine.evaluate_repository(&docs)?;

    let mut stdout = io::stdout();

    match cli.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&report)?;
            writeln!(stdout, "{}", json)?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_yaml::to_string(&report)?;
            writeln!(stdout, "{}", yaml)?;
        }
        _ => {
            println!("=== OVIS Pruning Engine Audit Report ===");
            println!("Evaluated Documents: {}", report.total_documents_evaluated);
            println!("Flagged Candidates:  {}", report.total_candidates_flagged);
            println!("Duplicates Found:    {}", report.total_duplicates_detected);
            println!("Dry Run Mode:        {}", report.dry_run);
            println!("\nFlagged Candidate Details:");

            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(vec![
                Cell::new("DOCUMENT ID").fg(Color::Red),
                Cell::new("TITLE").fg(Color::Red),
                Cell::new("FLAG REASONS").fg(Color::Red),
                Cell::new("DUPLICATE OF").fg(Color::Red),
            ]);

            for candidate in &report.candidates {
                let reasons = candidate.flag_reasons.iter().map(|r| format!("{:?}", r)).collect::<Vec<_>>().join(", ");
                let dup = candidate.duplicate_of.as_deref().unwrap_or("None");

                table.add_row(vec![
                    Cell::new(&candidate.document_id),
                    Cell::new(&candidate.title),
                    Cell::new(reasons),
                    Cell::new(dup),
                ]);
            }
            writeln!(stdout, "{}", table)?;
        }
    }

    if (!dry_run || force) && !report.dry_run {
        if let Some((pool, os, index)) = live {
            let deleted_count = engine
                .execute_pruning(&pool, &os, &index, &report)
                .await?;
            println!("[INFO] Executed batch cascading deletion: {} documents removed.", deleted_count);
        } else {
            println!("[WARN] No live database configured, so nothing was deleted.");
        }
    }

    Ok(())
}

fn get_sample_prune_docs() -> Vec<DocumentWithContent> {
    vec![
        DocumentWithContent {
            id: "https://docs.onyx.app/web/404".to_string(),
            semantic_id: "404 Not Found".to_string(),
            connector_id: 2,
            link: Some("https://docs.onyx.app/web/404".to_string()),
            content: "404 Not Found - The page you requested could not be located.".to_string(),
            updated_at: None,
            metadata: json!({}),
        },
        DocumentWithContent {
            id: "https://docs.onyx.app/web/dup1".to_string(),
            semantic_id: "Web Connector Documentation Spec".to_string(),
            connector_id: 2,
            link: Some("https://docs.onyx.app/web/dup1".to_string()),
            content: "The web connector allows Onyx to crawl and index HTTP websites using customizable depth limits and rate throttling.".to_string(),
            updated_at: None,
            metadata: json!({}),
        },
        DocumentWithContent {
            id: "https://docs.onyx.app/web/dup2".to_string(),
            semantic_id: "Web Connector Documentation Spec Copy".to_string(),
            connector_id: 2,
            link: Some("https://docs.onyx.app/web/dup2".to_string()),
            content: "The web connector allows Onyx to crawl and index HTTP websites using customizable depth limits and rate throttling.".to_string(),
            updated_at: None,
            metadata: json!({}),
        },
    ]
}

/// Handles the `tui` subcommand.
pub async fn handle_tui(cli: &Cli, connector: Option<i32>, search: Option<String>) -> Result<()> {
    let (docs, chunks) = if let Some(ref dsn) = cli.db_dsn {
        if let Ok(pool) = crate::compat::create_pg_pool(dsn).await {
            let fetched_docs = crate::compat::fetch_documents(
                &pool,
                connector,
                None,
                search.as_deref(),
                100,
                0,
            )
            .await
            .unwrap_or_else(|_| get_sample_documents());

            let mut fetched_chunks = Vec::new();
            if let Some(search_url) = cli.opensearch_url.as_deref() {
                let index = crate::compat::resolve_index(&pool).await?;
                let os = crate::compat::os_client(search_url)?;
                for doc in &fetched_docs {
                    if let Ok(doc_chunks) =
                        crate::compat::get_document_chunks(&os, &index, &doc.id).await
                    {
                        fetched_chunks.extend(doc_chunks);
                    }
                }
            }

            if fetched_chunks.is_empty() {
                fetched_chunks = get_sample_chunks();
            }

            (fetched_docs, fetched_chunks)
        } else {
            (get_sample_documents(), get_sample_chunks())
        }
    } else {
        (get_sample_documents(), get_sample_chunks())
    };

    run_tui(docs, chunks)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_data_integrity() {
        let docs = get_sample_documents();
        let chunks = get_sample_chunks();
        let conns = get_sample_connectors();

        assert!(!docs.is_empty());
        assert!(!chunks.is_empty());
        assert!(!conns.is_empty());
        assert_eq!(docs.len(), 5);
    }
}
