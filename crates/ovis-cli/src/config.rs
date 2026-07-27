//! Config file, profiles, and effective-value resolution.
//!
//! Precedence: **flags > env > profile > defaults** (`03_OUTPUT_AND_CONFIG.md`
//! §3). Every resolved value records where it came from, which is what
//! `ovis config show --origin` prints — a config system you cannot interrogate
//! is a config system you end up debugging with `strace`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::error::{CliError, CliResult};

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";

/// Where an effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Flag,
    Env(&'static str),
    Profile,
    File,
    Default,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Flag => write!(f, "flag"),
            Origin::Env(name) => write!(f, "env {name}"),
            Origin::Profile => write!(f, "profile"),
            Origin::File => write!(f, "config file"),
            Origin::Default => write!(f, "default"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Sourced<T> {
    pub value: T,
    pub origin: Origin,
}

impl<T> Sourced<T> {
    pub fn new(value: T, origin: Origin) -> Self {
        Self { value, origin }
    }
}

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub tui: TuiConfig,
    /// `ovis server start` reads its own section, so a homelab deploy is one
    /// file. Never merged into the client profile: a server secret has no
    /// business in the settings a client sends over the wire.
    #[serde(default)]
    pub server: ServerSection,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub server: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default)]
    pub pager: String,
    /// 0 = use the terminal width.
    #[serde(default)]
    pub table_max_width: u16,
    /// Set false to silence the "try the TUI" nudges.
    #[serde(default = "default_true")]
    pub hints: bool,
}

fn default_color() -> String {
    "auto".to_string()
}
fn default_true() -> bool {
    true
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            color: default_color(),
            pager: String::new(),
            table_max_width: 0,
            hints: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "default_refresh")]
    pub auto_refresh_secs: u64,
    #[serde(default = "default_screen")]
    pub default_screen: String,
}

fn default_refresh() -> u64 {
    5
}
fn default_screen() -> String {
    "pages".to_string()
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            auto_refresh_secs: default_refresh(),
            default_screen: default_screen(),
        }
    }
}

/// Settings for the backend `ovis server start` embeds. These are deliberately
/// separate from the client `[profiles]`: the client needs a URL, the server
/// needs credentials.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerSection {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database_url: Option<String>,
    pub opensearch_url: Option<String>,
    pub onyx_api_url: Option<String>,
    pub onyx_api_key: Option<String>,
    pub embed_api_url: Option<String>,
    pub api_token: Option<String>,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// `$OVIS_CONFIG`, else `$XDG_CONFIG_HOME/ovis/config.toml`, else the platform
/// config dir.
pub fn config_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("OVIS_CONFIG") {
        if !explicit.trim().is_empty() {
            return PathBuf::from(explicit);
        }
    }
    config_dir().join("config.toml")
}

pub fn config_dir() -> PathBuf {
    // XDG first even on macOS: someone who has set XDG_CONFIG_HOME means it.
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("ovis");
        }
    }
    directories::ProjectDirs::from("", "", "ovis")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".ovis"))
}

/// Where `@N` handles and one-a-day hint markers live.
pub fn state_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("ovis");
        }
    }
    directories::ProjectDirs::from("", "", "ovis")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".ovis"))
}

pub fn load_file(path: &Path) -> CliResult<ConfigFile> {
    if !path.exists() {
        return Ok(ConfigFile::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))
        .map_err(CliError::Other)?;
    // A malformed config file is something the user has to fix, so it is a
    // usage error rather than a generic one — and it names the file and the
    // offending line, which toml already does well.
    toml::from_str(&raw).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
}

pub fn save_file(path: &Path, cfg: &ConfigFile) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))
            .map_err(CliError::Other)?;
    }
    let text = toml::to_string_pretty(cfg)
        .context("serialising config")
        .map_err(CliError::Other)?;
    std::fs::write(path, text)
        .with_context(|| format!("writing {}", path.display()))
        .map_err(CliError::Other)?;
    restrict_permissions(path);
    Ok(())
}

/// The config file can hold a bearer token, so it is not world-readable.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Everything a command needs, with each value's provenance.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub path: PathBuf,
    pub file: ConfigFile,
    pub profile_name: Option<Sourced<String>>,
    pub server: Sourced<String>,
    pub token: Option<Sourced<String>>,
    pub color: Sourced<String>,
    pub pager: Option<Sourced<String>>,
    pub table_max_width: u16,
    pub hints: bool,
}

/// The flag-supplied half of the inputs.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub server: Option<String>,
    pub token: Option<String>,
    pub color: Option<String>,
    pub profile: Option<String>,
}

pub fn resolve(overrides: &Overrides) -> CliResult<Resolved> {
    let path = config_path();
    let file = load_file(&path)?;
    resolve_with(overrides, path, file, &EnvVars::from_process())
}

/// The environment, captured so tests can resolve without mutating the process.
#[derive(Debug, Clone, Default)]
pub struct EnvVars {
    pub server: Option<String>,
    pub token: Option<String>,
    pub profile: Option<String>,
    pub no_color: bool,
    pub pager: Option<String>,
}

impl EnvVars {
    pub fn from_process() -> Self {
        let non_empty = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Self {
            server: non_empty("OVIS_SERVER"),
            token: non_empty("OVIS_TOKEN"),
            profile: non_empty("OVIS_PROFILE"),
            // NO_COLOR's convention is "set to anything non-empty".
            no_color: std::env::var("NO_COLOR").is_ok_and(|v| !v.is_empty()),
            pager: non_empty("PAGER"),
        }
    }
}

pub fn resolve_with(
    overrides: &Overrides,
    path: PathBuf,
    file: ConfigFile,
    env: &EnvVars,
) -> CliResult<Resolved> {
    let profile_name = match (&overrides.profile, &env.profile, &file.default_profile) {
        (Some(p), _, _) => Some(Sourced::new(p.clone(), Origin::Flag)),
        (None, Some(p), _) => Some(Sourced::new(p.clone(), Origin::Env("OVIS_PROFILE"))),
        (None, None, Some(p)) => Some(Sourced::new(p.clone(), Origin::File)),
        _ => None,
    };

    // A profile named on the command line that does not exist is a mistake worth
    // stopping for — silently falling back to defaults would point the CLI at
    // the wrong server.
    let profile = match &profile_name {
        Some(name) => match file.profiles.get(&name.value) {
            Some(p) => Some(p.clone()),
            None if name.origin == Origin::Default => None,
            None => {
                let known: Vec<&str> = file.profiles.keys().map(String::as_str).collect();
                return Err(CliError::Usage(format!(
                    "no profile named '{}' in {}{}",
                    name.value,
                    path.display(),
                    if known.is_empty() {
                        " (the file defines none)".to_string()
                    } else {
                        format!(" (known: {})", known.join(", "))
                    }
                )));
            }
        },
        None => None,
    };

    let server = match (
        &overrides.server,
        &env.server,
        profile.as_ref().and_then(|p| p.server.as_ref()),
    ) {
        (Some(v), _, _) => Sourced::new(v.clone(), Origin::Flag),
        (None, Some(v), _) => Sourced::new(v.clone(), Origin::Env("OVIS_SERVER")),
        (None, None, Some(v)) => Sourced::new(v.clone(), Origin::Profile),
        _ => Sourced::new(DEFAULT_SERVER.to_string(), Origin::Default),
    };

    let token = match (
        &overrides.token,
        &env.token,
        profile.as_ref().and_then(|p| p.token.as_ref()),
    ) {
        (Some(v), _, _) => Some(Sourced::new(v.clone(), Origin::Flag)),
        (None, Some(v), _) => Some(Sourced::new(v.clone(), Origin::Env("OVIS_TOKEN"))),
        (None, None, Some(v)) => Some(Sourced::new(v.clone(), Origin::Profile)),
        _ => None,
    };

    let color = match (&overrides.color, env.no_color) {
        (Some(v), _) => Sourced::new(v.clone(), Origin::Flag),
        // NO_COLOR is a floor, not a preference: an explicit --color always wins.
        (None, true) => Sourced::new("never".to_string(), Origin::Env("NO_COLOR")),
        (None, false) if file.ui.color != "auto" => {
            Sourced::new(file.ui.color.clone(), Origin::File)
        }
        _ => Sourced::new("auto".to_string(), Origin::Default),
    };

    let pager = if !file.ui.pager.trim().is_empty() {
        Some(Sourced::new(file.ui.pager.clone(), Origin::File))
    } else {
        env.pager
            .clone()
            .map(|v| Sourced::new(v, Origin::Env("PAGER")))
    };

    Ok(Resolved {
        table_max_width: file.ui.table_max_width,
        hints: file.ui.hints,
        path,
        profile_name,
        server: Sourced::new(
            server.value.trim_end_matches('/').to_string(),
            server.origin,
        ),
        token,
        color,
        pager,
        file,
    })
}

/// The annotated file `ovis config init` writes.
pub const TEMPLATE: &str = r#"# OVIS CLI configuration.
#
# Precedence: command-line flags > environment > profile > defaults.
# `ovis config show --origin` prints each effective value and where it came from.

# Which [profiles.*] block to use when --profile is not given.
default_profile = "local"

[profiles.local]
server = "http://127.0.0.1:8080"

[profiles.homelab]
server = "http://192.168.4.113:8080"
# token = "…"           # only if the server sets OVIS_API_TOKEN

[ui]
color = "auto"          # auto | always | never  (NO_COLOR is always honoured)
pager = ""              # overrides $PAGER; empty means `less -RFX`
table_max_width = 0     # 0 = use the terminal width
hints = true            # occasional "try the TUI" nudges on stderr

[tui]
auto_refresh_secs = 5
default_screen = "pages"    # pages | connectors | activity

# `ovis server start` reads this section. It is separate from [profiles] on
# purpose: the client needs a URL, the server needs credentials, and the two
# should never be confused for one another.
# [server]
# host = "127.0.0.1"
# port = 8080
# database_url = "postgres://…@192.168.4.113:5433/postgres"
# opensearch_url = "http://192.168.4.113:9200"
# onyx_api_url = "http://192.168.4.113:8080"
# onyx_api_key = "…"      # `ovis server setup-onyx-key` writes this
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn file_with_profiles() -> ConfigFile {
        let mut file = ConfigFile {
            default_profile: Some("homelab".into()),
            ..Default::default()
        };
        file.profiles.insert(
            "homelab".into(),
            Profile {
                server: Some("http://gamma:8080".into()),
                token: Some("profile-token".into()),
            },
        );
        file.profiles.insert(
            "local".into(),
            Profile {
                server: Some("http://127.0.0.1:9999".into()),
                token: None,
            },
        );
        file
    }

    fn resolve_t(over: Overrides, file: ConfigFile, env: EnvVars) -> CliResult<Resolved> {
        resolve_with(&over, PathBuf::from("/tmp/config.toml"), file, &env)
    }

    #[test]
    fn precedence_is_flag_then_env_then_profile_then_default() {
        // default
        let r = resolve_t(
            Overrides::default(),
            ConfigFile::default(),
            EnvVars::default(),
        )
        .unwrap();
        assert_eq!(r.server.value, DEFAULT_SERVER);
        assert_eq!(r.server.origin, Origin::Default);

        // profile (via default_profile)
        let r = resolve_t(
            Overrides::default(),
            file_with_profiles(),
            EnvVars::default(),
        )
        .unwrap();
        assert_eq!(r.server.value, "http://gamma:8080");
        assert_eq!(r.server.origin, Origin::Profile);

        // env beats profile
        let env = EnvVars {
            server: Some("http://env:8080".into()),
            ..Default::default()
        };
        let r = resolve_t(Overrides::default(), file_with_profiles(), env.clone()).unwrap();
        assert_eq!(r.server.value, "http://env:8080");
        assert_eq!(r.server.origin, Origin::Env("OVIS_SERVER"));

        // flag beats env
        let over = Overrides {
            server: Some("http://flag:8080".into()),
            ..Default::default()
        };
        let r = resolve_t(over, file_with_profiles(), env).unwrap();
        assert_eq!(r.server.value, "http://flag:8080");
        assert_eq!(r.server.origin, Origin::Flag);
    }

    #[test]
    fn selecting_a_profile_switches_both_server_and_token() {
        let over = Overrides {
            profile: Some("local".into()),
            ..Default::default()
        };
        let r = resolve_t(over, file_with_profiles(), EnvVars::default()).unwrap();
        assert_eq!(r.server.value, "http://127.0.0.1:9999");
        assert!(r.token.is_none(), "the local profile sets no token");
    }

    #[test]
    fn an_unknown_profile_is_an_error_listing_the_known_ones() {
        let over = Overrides {
            profile: Some("typo".into()),
            ..Default::default()
        };
        let err = resolve_t(over, file_with_profiles(), EnvVars::default()).unwrap_err();
        let msg = err.message();
        // Silently falling back would point the CLI at the wrong server.
        assert!(msg.contains("typo"), "{msg}");
        assert!(msg.contains("homelab") && msg.contains("local"), "{msg}");
        // A mistyped --profile is a usage mistake, so exit 2 rather than 1.
        assert_eq!(err.exit_code(), crate::error::exit::USAGE);
    }

    #[test]
    fn no_color_forces_never_but_an_explicit_flag_still_wins() {
        let env = EnvVars {
            no_color: true,
            ..Default::default()
        };
        let r = resolve_t(Overrides::default(), ConfigFile::default(), env.clone()).unwrap();
        assert_eq!(r.color.value, "never");

        let over = Overrides {
            color: Some("always".into()),
            ..Default::default()
        };
        let r = resolve_t(over, ConfigFile::default(), env).unwrap();
        assert_eq!(r.color.value, "always");
        assert_eq!(r.color.origin, Origin::Flag);
    }

    #[test]
    fn a_trailing_slash_on_the_server_url_is_dropped_so_paths_do_not_double_up() {
        let over = Overrides {
            server: Some("http://gamma:8080/".into()),
            ..Default::default()
        };
        let r = resolve_t(over, ConfigFile::default(), EnvVars::default()).unwrap();
        assert_eq!(r.server.value, "http://gamma:8080");
    }

    #[test]
    fn the_written_template_round_trips() {
        let parsed: ConfigFile = toml::from_str(TEMPLATE).expect("template parses");
        assert_eq!(parsed.default_profile.as_deref(), Some("local"));
        assert!(parsed.profiles.contains_key("homelab"));
        assert_eq!(parsed.ui.color, "auto");
        assert_eq!(parsed.tui.auto_refresh_secs, 5);
        // And what we write back is parseable too.
        let round = toml::to_string_pretty(&parsed).unwrap();
        let again: ConfigFile = toml::from_str(&round).unwrap();
        assert_eq!(again.tui.default_screen, "pages");
    }

    #[test]
    fn the_server_section_is_never_folded_into_a_client_profile() {
        let file: ConfigFile = toml::from_str(
            r#"
            [profiles.p]
            server = "http://x:1"
            [server]
            onyx_api_key = "secret"
            database_url = "postgres://u:p@h/db"
            "#,
        )
        .unwrap();
        let over = Overrides {
            profile: Some("p".into()),
            ..Default::default()
        };
        let r = resolve_t(over, file, EnvVars::default()).unwrap();
        assert!(r.token.is_none());
        assert_eq!(r.server.value, "http://x:1");
        assert_eq!(r.file.server.onyx_api_key.as_deref(), Some("secret"));
    }
}
