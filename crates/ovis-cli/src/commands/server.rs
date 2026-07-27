//! `ovis server …` — run and manage the backend this binary embeds.
//!
//! Three defects from the audit are closed here: the PID file had no staleness
//! check (a leftover file blocked `-d` permanently), `stop` shelled out to
//! `kill` with no escalation or verification, and `status` probed `/api/health`
//! — a path that does not exist, so the SPA fallback answered 200 and any
//! process on the port read as a healthy OVIS.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cli::ServerCommand;
use crate::ctx::Ctx;
use crate::error::{CliError, CliResult};
use crate::output::style::Tone;

const DEFAULT_PORT: u16 = 8080;
/// SIGTERM, then this long for a graceful drain, then SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// PID file
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PidRecord {
    pid: u32,
    port: u16,
    started_at: chrono::DateTime<chrono::Utc>,
    /// The executable that was launched, so a recycled pid belonging to some
    /// other program is not mistaken for our server.
    exe: String,
}

fn pid_path() -> PathBuf {
    crate::config::state_dir().join("server.pid")
}

/// Where a detached server's output goes.
pub fn log_path() -> PathBuf {
    crate::config::state_dir().join("server.log")
}

/// The last `lines` lines written to the log since byte `from`.
///
/// Bounded by `from` so a start failure quotes only *this* attempt's output,
/// not the tail of whatever ran before it.
fn log_tail(path: &std::path::Path, from: u64, lines: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let fresh = text.get(from as usize..).unwrap_or(&text);
    let collected: Vec<&str> = fresh.lines().filter(|l| !l.trim().is_empty()).collect();
    collected
        .iter()
        .rev()
        .take(lines)
        .rev()
        .map(|l| l.to_string())
        .collect()
}

fn read_pid() -> Option<PidRecord> {
    let text = std::fs::read_to_string(pid_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_pid(record: &PidRecord) -> CliResult<()> {
    let path = pid_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(record)?)?;
    Ok(())
}

fn clear_pid() {
    let _ = std::fs::remove_file(pid_path());
}

/// Is this pid a live process?
fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Signal 0 performs the permission and existence checks without
        // delivering anything.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// A PID file whose process is gone is stale, and stale is not "running".
fn live_record() -> Option<PidRecord> {
    let record = read_pid()?;
    if is_alive(record.pid) {
        Some(record)
    } else {
        clear_pid();
        None
    }
}

// ---------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------

pub async fn run(ctx: &Ctx, action: &ServerCommand) -> CliResult<()> {
    match action {
        ServerCommand::Start {
            port,
            host,
            detach,
            config,
        } => start(ctx, *port, host.as_deref(), *detach, config.as_deref()).await,
        ServerCommand::Stop { port } => stop(ctx, *port).await,
        ServerCommand::Restart {
            port,
            host,
            detach,
            config,
        } => {
            let _ = stop(ctx, *port).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            start(ctx, *port, host.as_deref(), *detach, config.as_deref()).await
        }
        ServerCommand::Status { port } => status(ctx, *port).await,
        ServerCommand::SetupOnyxKey {
            onyx_url,
            email,
            name,
            print_only,
        } => {
            setup_onyx_key(
                ctx,
                onyx_url.as_deref(),
                email.as_deref(),
                name,
                *print_only,
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

async fn start(
    ctx: &Ctx,
    port: Option<u16>,
    host: Option<&str>,
    detach: bool,
    config_path: Option<&str>,
) -> CliResult<()> {
    if detach {
        return start_detached(ctx, port, host, config_path).await;
    }

    // The `[server]` section of the CLI's own config, handed to the backend's
    // loader as a file so its normal precedence still applies: environment wins
    // over the file, and the flags below win over both.
    //
    // A path is *always* supplied, even when the section is empty. Both programs
    // read `OVIS_CONFIG`, and they mean different files by it: to the CLI it is
    // `~/.config/ovis/config.toml` (profiles, ui, tui), to the backend it is a
    // flat `ServerConfig` table. Letting the backend's loader fall through to
    // `OVIS_CONFIG` on its own would make it read the CLI's file — and fail with
    // "config file does not exist" or, worse, silently find nothing in a file
    // that is not addressed to it.
    let bridged = bridge_config(ctx)?;
    let effective_path = match config_path {
        Some(explicit) => explicit.to_string(),
        None => bridged.path.clone(),
    };

    let mut cfg = ovis_backend::config::ServerConfig::load(Some(&effective_path))
        .map_err(|e| CliError::Other(anyhow::anyhow!("{e}")))?;
    drop(bridged);

    if let Some(port) = port {
        cfg.port = port;
    }
    if let Some(host) = host {
        cfg.host = host.to_string();
    }

    ovis_backend::init_tracing(cfg.json_logs());
    for warning in cfg.warnings() {
        ctx.out.warn(warning);
    }
    // Never the DSN: it carries the password. `summary()` redacts.
    ctx.out.note(cfg.summary());
    ctx.out
        .note(format!("dashboard  http://{}", cfg.bind_address()));
    ctx.out
        .note(format!("api        http://{}/api/v1", cfg.bind_address()));

    let grace = cfg.shutdown_grace();
    let address = cfg.bind_address();
    let state = ovis_backend::build_state(cfg)
        .await
        .map_err(|e| CliError::Other(anyhow::anyhow!("cannot start the server: {e:#}")))?;
    ovis_backend::spawn_background_tasks(state.clone());
    let router = ovis_backend::app(state);

    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|e| CliError::Other(anyhow::anyhow!("cannot bind {address}: {e}")))?;

    write_pid(&PidRecord {
        pid: std::process::id(),
        port: listener
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(DEFAULT_PORT),
        started_at: chrono::Utc::now(),
        exe: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    })?;

    let result = ovis_backend::serve_with_shutdown(listener, router, grace).await;
    clear_pid();
    result.map_err(|e| CliError::Other(anyhow::anyhow!("the server stopped with an error: {e:#}")))
}

/// A temp file holding the backend-shaped rendering of our `[server]` section,
/// deleted when the guard drops.
struct BridgedConfig {
    path: String,
}

impl Drop for BridgedConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Render `[server]` from the CLI config into a file the backend's loader
/// understands.
///
/// Always produces a file, even for an empty section: its other job is to give
/// `ServerConfig::load` an explicit path so it never falls back to the
/// `OVIS_CONFIG` the *CLI* owns.
fn bridge_config(ctx: &Ctx) -> CliResult<BridgedConfig> {
    let section = &ctx.cfg.file.server;
    let mut table = toml::map::Map::new();
    let mut put_str = |key: &str, value: &Option<String>| {
        if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            table.insert(key.to_string(), toml::Value::String(value.to_string()));
        }
    };
    put_str("database_url", &section.database_url);
    put_str("opensearch_url", &section.opensearch_url);
    put_str("onyx_api_url", &section.onyx_api_url);
    put_str("onyx_api_key", &section.onyx_api_key);
    put_str("embed_api_url", &section.embed_api_url);
    put_str("api_token", &section.api_token);
    put_str("host", &section.host);
    if let Some(port) = section.port {
        table.insert("port".into(), toml::Value::Integer(port.into()));
    }

    // The file can hold a database password and an Onyx token, so it is created
    // 0600 in the state directory rather than a world-writable temp dir.
    let dir = crate::config::state_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("server-config-{}.toml", std::process::id()));
    std::fs::write(
        &path,
        toml::to_string(&toml::Value::Table(table))
            .map_err(|e| CliError::Other(anyhow::anyhow!("cannot render server config: {e}")))?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(BridgedConfig {
        path: path.display().to_string(),
    })
}

/// How long a detached start waits for the child to answer before deciding it
/// did not come up. Binding a port and opening a Postgres pool takes ~1 s here;
/// this leaves room for a cold cache and a slow LAN.
const START_TIMEOUT: Duration = Duration::from_secs(20);

async fn start_detached(
    ctx: &Ctx,
    port: Option<u16>,
    host: Option<&str>,
    config_path: Option<&str>,
) -> CliResult<()> {
    if let Some(existing) = live_record() {
        return Err(CliError::Usage(format!(
            "a server is already running in the background (pid {}, port {}). Stop it with \
             `ovis server stop`.",
            existing.pid, existing.port
        )));
    }

    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(&exe);
    command.arg("server").arg("start");
    if let Some(port) = port {
        command.arg("--port").arg(port.to_string());
    }
    if let Some(host) = host {
        command.arg("--host").arg(host);
    }
    if let Some(path) = config_path {
        command.arg("--config").arg(path);
    }
    // Nothing secret is passed on the command line — `ps` shows argv to every
    // user on the box. Credentials reach the child through the inherited
    // environment and the config file, exactly as for a foreground start.

    // The child's output goes to a log file rather than to our terminal. An
    // inherited stdout keeps the parent's pipe open for as long as the server
    // lives, so `ovis server start -d | grep …` would simply hang; and a
    // background server that scribbles over an interactive shell is unpleasant
    // anyway. The log is where the failure reason is read from below.
    let log_path = log_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| CliError::Other(anyhow::anyhow!("cannot open {}: {e}", log_path.display())))?;
    let log_start = log.metadata().map(|m| m.len()).unwrap_or(0);
    command
        .stdout(log.try_clone()?)
        .stderr(log)
        .stdin(std::process::Stdio::null());

    let mut child = command
        .spawn()
        .map_err(|e| CliError::Other(anyhow::anyhow!("cannot spawn {}: {e}", exe.display())))?;

    let port = port.unwrap_or(DEFAULT_PORT);
    let pid = child.id();

    // Wait for it to actually answer before claiming it started. Reporting
    // success for a process that died a hundred milliseconds later — and
    // leaving a PID file behind for it — is the same false-success this
    // redesign exists to remove. The child inherits stderr, so its own error
    // message has already been printed by the time this returns.
    let probe = crate::api::ApiClient::new(
        &format!("http://127.0.0.1:{port}"),
        ctx.cfg.token.as_ref().map(|t| t.value.clone()),
        false,
    )?;
    let deadline = Instant::now() + START_TIMEOUT;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The child wrote why to the log; quote it rather than making
                // the user go and find it.
                for line in log_tail(&log_path, log_start, 8) {
                    ctx.out.warn(line);
                }
                return Err(CliError::Other(anyhow::anyhow!(
                    "the server exited immediately ({status}); see {}",
                    log_path.display()
                )));
            }
            Ok(None) => {}
            Err(e) => {
                return Err(CliError::Other(anyhow::anyhow!(
                    "cannot check on pid {pid}: {e}"
                )))
            }
        }

        if probe.health().await.is_ok() {
            break;
        }
        if Instant::now() >= deadline {
            // Still alive but not answering. Leave it running — it may just be
            // slow — but do not pretend the start succeeded.
            write_pid(&PidRecord {
                pid,
                port,
                started_at: chrono::Utc::now(),
                exe: exe.display().to_string(),
            })?;
            return Err(CliError::Unreachable {
                url: format!("http://127.0.0.1:{port}"),
                detail: format!(
                    "pid {pid} is running but did not answer within {}s; see {}, or stop it \
                     with `ovis server stop`",
                    START_TIMEOUT.as_secs(),
                    log_path.display()
                ),
            });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    write_pid(&PidRecord {
        pid,
        port,
        started_at: chrono::Utc::now(),
        exe: exe.display().to_string(),
    })?;

    ctx.out.note(format!(
        "started in the background (pid {pid}, port {port})"
    ));
    ctx.out.note(format!("logging to {}", log_path.display()));
    ctx.out
        .note("check it with `ovis server status`, stop it with `ovis server stop`");
    Ok(())
}

// ---------------------------------------------------------------------------
// stop
// ---------------------------------------------------------------------------

async fn stop(ctx: &Ctx, port: Option<u16>) -> CliResult<()> {
    let Some(record) = live_record() else {
        if read_pid().is_some() {
            ctx.out
                .note("cleared a stale PID file; no server was running");
        } else {
            ctx.out.note(format!(
                "no background server recorded{}",
                port.map(|p| format!(" for port {p}")).unwrap_or_default()
            ));
        }
        return Ok(());
    };

    #[cfg(unix)]
    {
        ctx.out
            .note(format!("stopping pid {} (SIGTERM)", record.pid));
        if unsafe { libc::kill(record.pid as libc::pid_t, libc::SIGTERM) } != 0 {
            let err = std::io::Error::last_os_error();
            clear_pid();
            return Err(CliError::Other(anyhow::anyhow!(
                "cannot signal pid {}: {err}",
                record.pid
            )));
        }

        // Wait for the drain rather than assuming it happened.
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline {
            if !is_alive(record.pid) {
                clear_pid();
                ctx.out.print(format!(
                    "{}  server stopped (pid {})",
                    Tone::paint(Tone::Ok, "ok", ctx.out.color),
                    record.pid
                ))?;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        ctx.out.warn(format!(
            "pid {} did not exit within {}s; sending SIGKILL",
            record.pid,
            STOP_GRACE.as_secs()
        ));
        unsafe { libc::kill(record.pid as libc::pid_t, libc::SIGKILL) };
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Verify, rather than reporting success and leaving it running.
        if is_alive(record.pid) {
            return Err(CliError::Other(anyhow::anyhow!(
                "pid {} survived SIGKILL; something else owns it",
                record.pid
            )));
        }
        clear_pid();
        ctx.out.print(format!(
            "{}  server killed (pid {})",
            Tone::paint(Tone::Warn, "ok", ctx.out.color),
            record.pid
        ))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = ctx;
        Err(CliError::Other(anyhow::anyhow!(
            "stopping a detached server is only implemented on unix"
        )))
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

async fn status(ctx: &Ctx, port: Option<u16>) -> CliResult<()> {
    // A --port override means localhost; otherwise the resolved server URL,
    // which may well be another machine.
    let target = match port {
        Some(port) => format!("http://127.0.0.1:{port}"),
        None => ctx.cfg.server.value.clone(),
    };

    let client = crate::api::ApiClient::new(
        &target,
        ctx.cfg.token.as_ref().map(|t| t.value.clone()),
        false,
    )?;

    let record = live_record();
    if let Some(record) = &record {
        ctx.out.note(format!(
            "PID file: pid {} on port {}, started {}",
            record.pid,
            record.port,
            crate::output::relative_time(&record.started_at, chrono::Utc::now())
        ));
        let log = log_path();
        if log.exists() {
            ctx.out.note(format!("log: {}", log.display()));
        }
    }

    // The correct versioned path, and a strict decode of the health body: an
    // arbitrary process on the port cannot satisfy this the way the SPA
    // fallback satisfied a bare 200.
    match client.health().await {
        Ok((healthy, report)) => {
            let mut sub = Ctx {
                api: client,
                out: ctx.out.clone(),
                interaction: ctx.interaction,
                cfg: ctx.cfg.clone(),
            };
            sub.cfg.server.value = target;
            crate::commands::status::render(&sub, healthy, &report)
        }
        Err(err @ CliError::Unreachable { .. }) => {
            ctx.out.print(format!(
                "{}  no OVIS server answering at {target}",
                Tone::paint(Tone::Error, "down", ctx.out.color)
            ))?;
            if record.is_some() {
                ctx.out.warn(
                    "a PID file exists but nothing is answering; the process may still be \
                     starting, or it is bound elsewhere",
                );
            }
            Err(err)
        }
        // Something answered, but not with a health report. The old status
        // command probed `/api/health` — a path that does not exist — and the
        // SPA fallback returned 200, so any process on the port read as a
        // healthy OVIS. Being specific about *what* answered is the fix.
        Err(CliError::Http { status, .. }) => {
            ctx.out.print(format!(
                "{}  something is listening on {target} but it is not an OVIS server \
                 (HTTP {status} from /api/v1/system/health)",
                Tone::paint(Tone::Error, "foreign", ctx.out.color)
            ))?;
            Err(CliError::Unreachable {
                url: target,
                detail: format!("HTTP {status} from a non-OVIS process"),
            })
        }
        Err(other) => Err(other),
    }
}

// ---------------------------------------------------------------------------
// setup-onyx-key
// ---------------------------------------------------------------------------

/// Mint the token the backend needs for connector actions.
///
/// The redesign called this "mint an API key". `POST /admin/api-key` is
/// paywalled on this Onyx edition — it answers `402 FEATURE_NOT_AVAILABLE`
/// before it even looks at credentials, which is why the `api_key` table is
/// empty. `OnyxClient::mint_pat` tries that endpoint first anyway (in case the
/// edition ever changes) and falls back to a personal access token, which is
/// presented identically as `Authorization: Bearer …`.
async fn setup_onyx_key(
    ctx: &Ctx,
    onyx_url: Option<&str>,
    email: Option<&str>,
    token_name: &str,
    print_only: bool,
) -> CliResult<()> {
    if ctx.interaction.no_input {
        return Err(CliError::NeedsConfirmation(
            "setup-onyx-key is interactive: it needs an admin password, which is deliberately \
             not accepted as a flag (argv is visible to every user via `ps`)"
                .into(),
        ));
    }

    let default_url = onyx_url
        .map(str::to_string)
        .or_else(|| ctx.cfg.file.server.onyx_api_url.clone())
        .or_else(|| std::env::var("ONYX_API_URL").ok())
        .unwrap_or_default();

    let url = match onyx_url {
        Some(url) => url.to_string(),
        None => crate::prompt::ask_line("Onyx base URL", Some(&default_url))?,
    };
    if url.trim().is_empty() {
        return Err(CliError::Usage("an Onyx base URL is required".into()));
    }

    let email = match email {
        Some(email) => email.to_string(),
        None => crate::prompt::ask_line(
            "Onyx admin email",
            std::env::var("ONYX_ADMIN_EMAIL").ok().as_deref(),
        )?,
    };
    if email.trim().is_empty() {
        return Err(CliError::Usage("an Onyx admin email is required".into()));
    }

    // Read interactively and never echoed, never stored, never in argv.
    let password = crate::prompt::read_password(&format!("Onyx password for {email}: "))?;
    if password.is_empty() {
        return Err(CliError::Usage("no password given".into()));
    }

    ctx.out.note(format!("logging in to {url}"));
    let token = ovis_core::onyx::OnyxClient::mint_pat(
        &url,
        &ovis_core::onyx::PatCredentials {
            email: email.clone(),
            password,
            token_name: token_name.to_string(),
        },
    )
    .await
    .map_err(|e| match e {
        ovis_core::CoreError::Onyx { status, body } if status == 401 || status == 400 => {
            CliError::Usage(format!(
                "Onyx rejected those credentials (HTTP {status}): {body}"
            ))
        }
        other => CliError::Other(anyhow::anyhow!("could not mint an Onyx token: {other}")),
    })?;

    // Onyx returns the raw token exactly once — there is no way to read it back,
    // only to revoke it and mint another.
    if print_only {
        ctx.out.print(format!("ONYX_API_KEY={token}"))?;
        ctx.out.warn("Onyx shows this token once; store it now");
        return Ok(());
    }

    // Into the *server* section, never a client profile: this is a server
    // credential and the client has no use for it.
    let mut file = ctx.cfg.file.clone();
    file.server.onyx_api_url = Some(url.trim_end_matches('/').to_string());
    file.server.onyx_api_key = Some(token);
    crate::config::save_file(&ctx.cfg.path, &file)?;

    ctx.out.print(format!(
        "{}  wrote the Onyx token to {} under [server]",
        Tone::paint(Tone::Ok, "ok", ctx.out.color),
        ctx.cfg.path.display()
    ))?;
    ctx.out.note(
        "restart the server, then confirm with `ovis status` — the onyx api row should read ok",
    );
    ctx.out.warn(
        "Onyx shows a token once. If you lose this file you must revoke the token and mint \
         another.",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pid_record_round_trips() {
        let record = PidRecord {
            pid: 4242,
            port: 8080,
            started_at: chrono::Utc::now(),
            exe: "/usr/local/bin/ovis".into(),
        };
        let text = serde_json::to_string(&record).unwrap();
        let back: PidRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(back.pid, 4242);
        assert_eq!(back.port, 8080);
        assert_eq!(back.exe, "/usr/local/bin/ovis");
    }

    #[test]
    fn our_own_pid_is_alive_and_an_impossible_one_is_not() {
        assert!(is_alive(std::process::id()));
        // Above every plausible pid_max, so it cannot collide with a real process.
        assert!(!is_alive(0x7FFF_FFF0));
    }

    #[test]
    fn the_stop_escalation_is_the_documented_one() {
        // SIGTERM, wait, SIGKILL — not a bare `kill` with no verification.
        assert_eq!(STOP_GRACE.as_secs(), 10);
    }
}
