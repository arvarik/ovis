//! Server configuration.
//!
//! Everything comes from the environment, optionally layered over a TOML file
//! (`--config ovis.toml` or `OVIS_CONFIG`), with the environment winning. There
//! are **no credentials in source**: the previous build compiled the production
//! Postgres DSN — password included — into every binary and printed it to stdout
//! on startup.
//!
//! Invalid values are startup errors, not silent fallbacks. A typo'd
//! `OVIS_PORT` used to become 8080 and leave someone wondering why nothing
//! answered on the port they asked for.

use std::time::Duration;

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

/// Fields are `Option` where "unset" is meaningful and disables a feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,

    /// Direct Postgres, port 5433 in this deployment — **not** pgbouncer on
    /// 5432. SQLx uses prepared statements; pgbouncer's transaction pooling
    /// breaks them.
    pub database_url: String,
    pub db_max_connections: u32,

    pub opensearch_url: String,
    pub opensearch_username: Option<String>,
    pub opensearch_password: Option<String>,

    /// Unset ⇒ every action endpoint answers `503 ONYX_UNCONFIGURED`.
    pub onyx_api_url: Option<String>,
    /// An Onyx API key or personal access token. See
    /// `ovis_core::onyx::OnyxClient::mint_pat`.
    pub onyx_api_key: Option<String>,

    /// Unset ⇒ `mode=hybrid|semantic` search degrades to BM25 and says so.
    pub embed_api_url: Option<String>,
    pub embed_model: String,

    /// When set, `/api/v1/*` requires `Authorization: Bearer <token>`.
    /// `/system/health` stays open so container probes keep working.
    pub api_token: Option<String>,
    pub cors_origins: String,

    pub max_page_size: i64,
    pub batch_delete_max: usize,
    pub max_stream_limit: i64,

    /// Seconds between `search_settings` re-reads, which is how a re-embed
    /// switchover gets picked up.
    pub runtime_refresh_secs: u64,
    /// Seconds between attempts to drain `ovis.pending_index_deletes`.
    pub pending_delete_drain_secs: u64,

    pub request_timeout_secs: u64,
    pub body_limit_bytes: usize,
    pub shutdown_grace_secs: u64,

    pub log_format: String,

    /// Days a staged (hidden) document waits before the reaper may delete it.
    /// 0 is allowed — deletion is then due at the next reaper cycle — but
    /// clients require the typed-count confirmation for it everywhere.
    pub prune_grace_days: i64,
    /// Seconds between reaper cycles.
    pub prune_reaper_interval_secs: u64,
    /// Documents per reaper batch. Clamped to `batch_delete_max`.
    pub prune_reaper_batch_size: usize,
    /// Hard ceiling on reaper deletions per trailing hour. The 375 GB index
    /// has tripped disk watermarks before; deletion pressure stays gentle.
    pub prune_max_docs_per_hour: i64,
    /// Bulk mutations larger than this require the typed count on both
    /// surfaces.
    pub prune_big_batch: i64,
    /// Documents per scan page (keyset batch size).
    pub prune_scan_page_size: i64,
    /// Milliseconds the reaper sleeps between delete batches.
    pub prune_reaper_pause_ms: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: 8080,
            // Deliberately empty: there is no sane default for someone else's
            // production database, and guessing one is how a hardcoded
            // credential gets into a binary.
            database_url: String::new(),
            db_max_connections: 20,
            opensearch_url: String::new(),
            opensearch_username: None,
            opensearch_password: None,
            onyx_api_url: None,
            onyx_api_key: None,
            embed_api_url: None,
            embed_model: "snowflake-arctic-embed:m".into(),
            api_token: None,
            cors_origins: "*".into(),
            max_page_size: 500,
            batch_delete_max: 1000,
            max_stream_limit: 10_000,
            runtime_refresh_secs: 60,
            pending_delete_drain_secs: 60,
            request_timeout_secs: 30,
            body_limit_bytes: 2 * 1024 * 1024,
            shutdown_grace_secs: 10,
            log_format: "text".into(),
            prune_grace_days: 7,
            prune_reaper_interval_secs: 300,
            prune_reaper_batch_size: 100,
            prune_max_docs_per_hour: 2000,
            prune_big_batch: 500,
            prune_scan_page_size: 1000,
            prune_reaper_pause_ms: 2000,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("configuration error: {0}")]
pub struct ConfigError(pub String);

impl ServerConfig {
    /// Load from an optional TOML file, then the environment.
    ///
    /// `DATABASE_URL` and `OPENSEARCH_URL` keep their conventional names; every
    /// OVIS-specific setting is `OVIS_`-prefixed.
    pub fn load(config_path: Option<&str>) -> Result<Self, ConfigError> {
        let mut figment = Figment::from(Serialized::defaults(ServerConfig::default()));

        let path = config_path
            .map(|p| p.to_string())
            .or_else(|| std::env::var("OVIS_CONFIG").ok());
        if let Some(path) = path {
            if !std::path::Path::new(&path).exists() {
                return Err(ConfigError(format!("config file '{path}' does not exist")));
            }
            figment = figment.merge(Toml::file(&path));
        }

        // Names that predate the prefix convention and that operators expect.
        // Applied before the OVIS_ layer so an explicit OVIS_ setting still wins.
        for (env_key, field) in [
            ("DATABASE_URL", "database_url"),
            ("OPENSEARCH_URL", "opensearch_url"),
            ("OPENSEARCH_USERNAME", "opensearch_username"),
            ("OPENSEARCH_PASSWORD", "opensearch_password"),
            ("ONYX_API_URL", "onyx_api_url"),
            ("ONYX_API_KEY", "onyx_api_key"),
            ("EMBED_API_URL", "embed_api_url"),
            ("EMBED_MODEL", "embed_model"),
        ] {
            if let Ok(value) = std::env::var(env_key) {
                if !value.is_empty() {
                    figment = figment.merge(Serialized::default(field, value));
                }
            }
        }

        // OVIS_HOST -> host, OVIS_MAX_PAGE_SIZE -> max_page_size, and so on.
        figment = figment.merge(Env::prefixed("OVIS_").ignore(&["CONFIG"]));

        let mut config: ServerConfig = figment.extract().map_err(|e| {
            // figment reports the offending key, which is what makes a bad
            // OVIS_PORT actionable instead of mysterious.
            ConfigError(e.to_string())
        })?;

        config.normalise();
        config.validate()?;
        Ok(config)
    }

    /// An optional setting that is present but blank means "unset".
    ///
    /// `.env` files habitually carry `OVIS_API_TOKEN=` as a placeholder, and
    /// `set -a; . ./.env` exports it as an empty string. Taken literally that
    /// would *enable* bearer auth with the empty token — which any caller can
    /// satisfy by sending `Authorization: Bearer `, i.e. worse than no auth at
    /// all, while looking enabled in the startup log.
    fn normalise(&mut self) {
        fn blank_to_none(field: &mut Option<String>) {
            if field.as_deref().map(str::trim).is_some_and(str::is_empty) {
                *field = None;
            }
        }
        blank_to_none(&mut self.api_token);
        blank_to_none(&mut self.onyx_api_url);
        blank_to_none(&mut self.onyx_api_key);
        blank_to_none(&mut self.embed_api_url);
        blank_to_none(&mut self.opensearch_username);
        blank_to_none(&mut self.opensearch_password);
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.database_url.trim().is_empty() {
            return Err(ConfigError(
                "DATABASE_URL is required. Point it at Postgres directly (port 5433 in the \
                 homelab deployment), not at pgbouncer — SQLx uses prepared statements and \
                 pgbouncer's transaction pooling breaks them."
                    .into(),
            ));
        }
        if self.opensearch_url.trim().is_empty() {
            return Err(ConfigError(
                "OPENSEARCH_URL is required (e.g. http://192.168.4.113:9200).".into(),
            ));
        }
        if self.port == 0 {
            return Err(ConfigError("OVIS_PORT must not be 0.".into()));
        }
        if self.max_page_size < 1 {
            return Err(ConfigError("OVIS_MAX_PAGE_SIZE must be at least 1.".into()));
        }
        if self.batch_delete_max < 1 {
            return Err(ConfigError(
                "OVIS_BATCH_DELETE_MAX must be at least 1.".into(),
            ));
        }
        if !(0..=90).contains(&self.prune_grace_days) {
            return Err(ConfigError(
                "OVIS_PRUNE_GRACE_DAYS must be between 0 and 90.".into(),
            ));
        }
        if self.prune_reaper_batch_size < 1 {
            return Err(ConfigError(
                "OVIS_PRUNE_REAPER_BATCH_SIZE must be at least 1.".into(),
            ));
        }
        if self.prune_max_docs_per_hour < 1 {
            return Err(ConfigError(
                "OVIS_PRUNE_MAX_DOCS_PER_HOUR must be at least 1.".into(),
            ));
        }
        if self.prune_big_batch < 1 {
            return Err(ConfigError("OVIS_PRUNE_BIG_BATCH must be at least 1.".into()));
        }
        if self.prune_scan_page_size < 1 {
            return Err(ConfigError(
                "OVIS_PRUNE_SCAN_PAGE_SIZE must be at least 1.".into(),
            ));
        }
        Ok(())
    }

    /// The reaper batch size, never above the batch-delete ceiling.
    pub fn prune_reaper_batch(&self) -> usize {
        self.prune_reaper_batch_size.min(self.batch_delete_max)
    }

    /// Warnings worth saying out loud once the logger exists.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.onyx_api_url.is_some() && self.onyx_api_key.is_none() {
            warnings.push(
                "ONYX_API_URL is set but ONYX_API_KEY is not; connector actions will return \
                 503 ONYX_UNCONFIGURED. See the README for how to mint a token — note that \
                 POST /admin/api-key is paywalled on the Onyx free tier, so a personal access \
                 token is used instead."
                    .to_string(),
            );
        }
        if self.database_url.contains(":5432") {
            warnings.push(
                "DATABASE_URL points at port 5432, which is pgbouncer in the homelab \
                 deployment. SQLx uses prepared statements and pgbouncer runs transaction \
                 pooling, which breaks them — use the direct Postgres port (5433)."
                    .to_string(),
            );
        }
        if self.api_token.is_none() && self.cors_origin_list().is_none() {
            warnings.push(
                "no OVIS_API_TOKEN and CORS allows any origin; destructive endpoints are open \
                 to anything that can reach this port. Fine on a trusted LAN, not beyond it."
                    .to_string(),
            );
        }
        warnings
    }

    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// `None` means "any origin".
    pub fn cors_origin_list(&self) -> Option<Vec<String>> {
        let trimmed = self.cors_origins.trim();
        if trimmed == "*" || trimmed.is_empty() {
            return None;
        }
        Some(
            trimmed
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        )
    }

    pub fn onyx_configured(&self) -> bool {
        self.onyx_api_url
            .as_ref()
            .is_some_and(|u| !u.trim().is_empty())
            && self
                .onyx_api_key
                .as_ref()
                .is_some_and(|k| !k.trim().is_empty())
    }

    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }

    pub fn shutdown_grace(&self) -> Duration {
        Duration::from_secs(self.shutdown_grace_secs)
    }

    pub fn json_logs(&self) -> bool {
        self.log_format.eq_ignore_ascii_case("json")
    }

    /// Redacted rendering, safe to log at startup.
    pub fn summary(&self) -> String {
        format!(
            "bind={} db={} opensearch={} onyx={} embedder={} auth={} cors={}",
            self.bind_address(),
            redact_dsn(&self.database_url),
            self.opensearch_url,
            if self.onyx_configured() {
                "configured"
            } else {
                "unconfigured"
            },
            self.embed_api_url.as_deref().unwrap_or("unconfigured"),
            if self.api_token.is_some() {
                "bearer"
            } else {
                "open"
            },
            self.cors_origins,
        )
    }
}

/// Strip the password from a DSN so it can be logged.
pub fn redact_dsn(dsn: &str) -> String {
    // postgres://user:password@host:port/db  ->  postgres://user:***@host:port/db
    let Some((scheme, rest)) = dsn.split_once("://") else {
        return "<malformed>".into();
    };
    let Some((userinfo, hostpart)) = rest.split_once('@') else {
        return format!("{scheme}://{rest}");
    };
    let user = userinfo.split(':').next().unwrap_or("");
    if userinfo.contains(':') {
        format!("{scheme}://{user}:***@{hostpart}")
    } else {
        format!("{scheme}://{userinfo}@{hostpart}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Environment variables are process-global; serialise the tests that touch
    /// them so they cannot interleave.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const MANAGED_KEYS: &[&str] = &[
        "DATABASE_URL",
        "OPENSEARCH_URL",
        "OPENSEARCH_USERNAME",
        "OPENSEARCH_PASSWORD",
        "ONYX_API_URL",
        "ONYX_API_KEY",
        "EMBED_API_URL",
        "EMBED_MODEL",
        "OVIS_CONFIG",
        "OVIS_PORT",
        "OVIS_HOST",
        "OVIS_MAX_PAGE_SIZE",
        "OVIS_API_TOKEN",
    ];

    struct EnvGuard;

    impl EnvGuard {
        fn with(vars: &[(&str, &str)]) -> Self {
            for k in MANAGED_KEYS {
                std::env::remove_var(k);
            }
            for (k, v) in vars {
                std::env::set_var(k, v);
            }
            EnvGuard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for k in MANAGED_KEYS {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    fn requires_a_database_url_and_says_why_pgbouncer_is_wrong() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::with(&[("OPENSEARCH_URL", "http://os:9200")]);
        let err = ServerConfig::load(None).unwrap_err();
        assert!(err.to_string().contains("DATABASE_URL is required"));
        assert!(err.to_string().contains("pgbouncer"));
    }

    #[test]
    fn requires_an_opensearch_url() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::with(&[("DATABASE_URL", "postgres://u:p@h:5433/db")]);
        let err = ServerConfig::load(None).unwrap_err();
        assert!(err.to_string().contains("OPENSEARCH_URL is required"));
    }

    #[test]
    fn a_bad_port_is_a_startup_error_not_a_silent_8080() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::with(&[
            ("DATABASE_URL", "postgres://u:p@h:5433/db"),
            ("OPENSEARCH_URL", "http://os:9200"),
            ("OVIS_PORT", "not-a-number"),
        ]);
        let err = ServerConfig::load(None).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("port"),
            "the error should name the offending key: {err}"
        );
    }

    #[test]
    fn env_overrides_are_picked_up_under_both_naming_conventions() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::with(&[
            ("DATABASE_URL", "postgres://u:p@h:5433/db"),
            ("OPENSEARCH_URL", "http://os:9200"),
            ("OVIS_PORT", "9999"),
            ("OVIS_MAX_PAGE_SIZE", "250"),
            ("ONYX_API_URL", "http://onyx:8080"),
            ("ONYX_API_KEY", "tok"),
            ("EMBED_API_URL", "http://embed:8090"),
        ]);
        let cfg = ServerConfig::load(None).unwrap();
        assert_eq!(cfg.port, 9999);
        assert_eq!(cfg.max_page_size, 250);
        assert_eq!(cfg.database_url, "postgres://u:p@h:5433/db");
        assert_eq!(cfg.opensearch_url, "http://os:9200");
        assert_eq!(cfg.onyx_api_url.as_deref(), Some("http://onyx:8080"));
        assert_eq!(cfg.embed_api_url.as_deref(), Some("http://embed:8090"));
        assert!(cfg.onyx_configured());
    }

    #[test]
    fn a_toml_file_supplies_values_and_env_still_wins() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::with(&[]);
        let dir = std::env::temp_dir().join(format!("ovis-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ovis.toml");
        std::fs::write(
            &path,
            "database_url = \"postgres://from:toml@h:5433/db\"\n\
             opensearch_url = \"http://from-toml:9200\"\n\
             port = 7777\n\
             max_page_size = 111\n",
        )
        .unwrap();

        let cfg = ServerConfig::load(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(cfg.port, 7777);
        assert_eq!(cfg.max_page_size, 111);
        assert_eq!(cfg.opensearch_url, "http://from-toml:9200");

        std::env::set_var("OVIS_PORT", "8888");
        let cfg = ServerConfig::load(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(cfg.port, 8888, "the environment must win over the file");
        assert_eq!(cfg.max_page_size, 111);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_config_file_is_an_error_not_a_shrug() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::with(&[]);
        let err = ServerConfig::load(Some("/nonexistent/ovis.toml")).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn onyx_is_only_configured_with_both_url_and_key() {
        let base = ServerConfig {
            database_url: "postgres://u:p@h/db".into(),
            opensearch_url: "http://os:9200".into(),
            ..Default::default()
        };
        assert!(!base.onyx_configured());
        assert!(!ServerConfig {
            onyx_api_url: Some("http://onyx:8080".into()),
            ..base.clone()
        }
        .onyx_configured());
        assert!(!ServerConfig {
            onyx_api_key: Some("k".into()),
            ..base.clone()
        }
        .onyx_configured());
        assert!(ServerConfig {
            onyx_api_url: Some("http://onyx:8080".into()),
            onyx_api_key: Some("k".into()),
            ..base.clone()
        }
        .onyx_configured());
        // Empty strings do not count as configured.
        assert!(!ServerConfig {
            onyx_api_url: Some("  ".into()),
            onyx_api_key: Some("k".into()),
            ..base
        }
        .onyx_configured());
    }

    #[test]
    fn pgbouncer_and_open_access_produce_warnings() {
        let cfg = ServerConfig {
            database_url: "postgres://u:p@gamma:5432/postgres".into(),
            opensearch_url: "http://os:9200".into(),
            ..Default::default()
        };
        let warnings = cfg.warnings().join(" | ");
        assert!(warnings.contains("pgbouncer"));
        assert!(warnings.contains("any origin"));

        let locked_down = ServerConfig {
            database_url: "postgres://u:p@gamma:5433/postgres".into(),
            opensearch_url: "http://os:9200".into(),
            api_token: Some("t".into()),
            cors_origins: "https://ovis.example".into(),
            ..Default::default()
        };
        assert!(locked_down.warnings().is_empty());
    }

    #[test]
    fn cors_origin_parsing() {
        let mut cfg = ServerConfig::default();
        assert_eq!(cfg.cors_origin_list(), None);
        cfg.cors_origins = "  ".into();
        assert_eq!(cfg.cors_origin_list(), None);
        cfg.cors_origins = "https://a.example, https://b.example ,".into();
        assert_eq!(
            cfg.cors_origin_list(),
            Some(vec![
                "https://a.example".to_string(),
                "https://b.example".to_string()
            ])
        );
    }

    #[test]
    fn dsn_redaction_never_leaks_the_password() {
        assert_eq!(
            redact_dsn("postgres://postgres:hunter2@192.168.4.113:5433/postgres"),
            "postgres://postgres:***@192.168.4.113:5433/postgres"
        );
        assert_eq!(
            redact_dsn("postgres://postgres@host/db"),
            "postgres://postgres@host/db"
        );
        assert_eq!(redact_dsn("postgres://host/db"), "postgres://host/db");
        assert_eq!(redact_dsn("garbage"), "<malformed>");
    }

    #[test]
    fn the_startup_summary_is_safe_to_log() {
        let cfg = ServerConfig {
            database_url: "postgres://postgres:hunter2@gamma:5433/postgres".into(),
            opensearch_url: "http://gamma:9200".into(),
            api_token: Some("secret-token".into()),
            onyx_api_key: Some("secret-key".into()),
            onyx_api_url: Some("http://gamma:8080".into()),
            ..Default::default()
        };
        let summary = cfg.summary();
        assert!(!summary.contains("hunter2"));
        assert!(!summary.contains("secret-token"));
        assert!(!summary.contains("secret-key"));
        assert!(summary.contains("auth=bearer"));
        assert!(summary.contains("onyx=configured"));
    }

    #[test]
    fn a_blank_optional_setting_means_unset_not_enabled() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Exactly what `set -a; . ./.env` does with a placeholder line.
        let _env = EnvGuard::with(&[
            ("DATABASE_URL", "postgres://u:p@h:5433/db"),
            ("OPENSEARCH_URL", "http://os:9200"),
            ("OVIS_API_TOKEN", ""),
            ("ONYX_API_URL", ""),
            ("ONYX_API_KEY", "   "),
        ]);
        let cfg = ServerConfig::load(None).unwrap();
        assert!(
            cfg.api_token.is_none(),
            "an empty token would enable auth that `Authorization: Bearer ` satisfies"
        );
        assert!(cfg.onyx_api_url.is_none());
        assert!(cfg.onyx_api_key.is_none());
        assert!(!cfg.onyx_configured());
        assert!(cfg.summary().contains("auth=open"));
    }

    #[test]
    fn defaults_carry_no_credentials_at_all() {
        let cfg = ServerConfig::default();
        assert!(cfg.database_url.is_empty());
        assert!(cfg.opensearch_url.is_empty());
        assert!(cfg.onyx_api_key.is_none());
        assert!(cfg.api_token.is_none());
        // Field *names* mention passwords; no field may carry a value.
        assert!(cfg.opensearch_password.is_none());
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("192.168."), "{rendered}");
        assert!(!rendered.contains("postgres://"), "{rendered}");
    }
}
