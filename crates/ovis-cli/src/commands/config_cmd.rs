//! `ovis config …` — inspect and edit the config file.

use crate::cli::ConfigCommand;
use crate::config::{self, ConfigFile};
use crate::ctx::Ctx;
use crate::error::{usage, CliError, CliResult};
use crate::output::style::Tone;
use crate::output::table::{Grid, GridCell};
use crate::output::Format;

pub fn run(ctx: &Ctx, action: &ConfigCommand) -> CliResult<()> {
    match action {
        ConfigCommand::Init { force } => init(ctx, *force),
        ConfigCommand::Show { origin } => show(ctx, *origin),
        ConfigCommand::Set { key, value } => set(ctx, key, value),
        ConfigCommand::Path => ctx.out.print(ctx.cfg.path.display().to_string()),
    }
}

fn init(ctx: &Ctx, force: bool) -> CliResult<()> {
    let path = &ctx.cfg.path;
    if path.exists() && !force {
        return Err(CliError::Usage(format!(
            "{} already exists; pass --force to overwrite it",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, config::TEMPLATE)?;

    ctx.out.print(format!("wrote {}", path.display()))?;
    // The template carries no secret; anything that later does is written 0600
    // by `config set` and `server setup-onyx-key`.
    ctx.out
        .note("no credentials were written; add a token only if the server sets OVIS_API_TOKEN");
    Ok(())
}

fn show(ctx: &Ctx, origin: bool) -> CliResult<()> {
    if matches!(ctx.out.format, Format::Json | Format::Yaml) {
        // The file as-is, with secrets masked — `config show` is something
        // people paste into issues.
        let mut file = ctx.cfg.file.clone();
        mask(&mut file);
        return match ctx.out.format {
            Format::Json => ctx.out.json(&file),
            _ => ctx.out.yaml(&file),
        };
    }

    let mut grid = if origin {
        Grid::new(vec!["SETTING".into(), "VALUE".into(), "FROM".into()])
    } else {
        Grid::new(vec!["SETTING".into(), "VALUE".into()])
    };

    let mut row = |name: &str, value: String, from: String, tone: Tone| {
        let mut cells = vec![GridCell::plain(name), GridCell::toned(value, tone)];
        if origin {
            cells.push(GridCell::toned(from, Tone::Dim));
        }
        grid.push(cells);
    };

    row(
        "config file",
        ctx.cfg.path.display().to_string(),
        if ctx.cfg.path.exists() {
            "exists".into()
        } else {
            "not created yet".into()
        },
        if ctx.cfg.path.exists() {
            Tone::Plain
        } else {
            Tone::Dim
        },
    );
    row(
        "profile",
        ctx.cfg
            .profile_name
            .as_ref()
            .map(|p| p.value.clone())
            .unwrap_or_else(|| "(none)".into()),
        ctx.cfg
            .profile_name
            .as_ref()
            .map(|p| p.origin.to_string())
            .unwrap_or_else(|| "default".into()),
        Tone::Plain,
    );
    row(
        "server",
        ctx.cfg.server.value.clone(),
        ctx.cfg.server.origin.to_string(),
        Tone::Plain,
    );
    row(
        "token",
        match &ctx.cfg.token {
            Some(_) => "set".into(),
            None => "(none)".into(),
        },
        ctx.cfg
            .token
            .as_ref()
            .map(|t| t.origin.to_string())
            .unwrap_or_else(|| "default".into()),
        Tone::Dim,
    );
    row(
        "color",
        ctx.cfg.color.value.clone(),
        ctx.cfg.color.origin.to_string(),
        Tone::Plain,
    );
    row(
        "pager",
        ctx.cfg
            .pager
            .as_ref()
            .map(|p| p.value.clone())
            .unwrap_or_else(|| "less -RFX".into()),
        ctx.cfg
            .pager
            .as_ref()
            .map(|p| p.origin.to_string())
            .unwrap_or_else(|| "default".into()),
        Tone::Plain,
    );
    row(
        "ui.hints",
        ctx.cfg.hints.to_string(),
        "config file".into(),
        Tone::Plain,
    );
    row(
        "server section",
        if ctx.cfg.file.server.database_url.is_some() {
            "configured (used by `ovis server start`)".into()
        } else {
            "(none)".into()
        },
        "config file".into(),
        Tone::Dim,
    );

    ctx.out.grid(&grid)?;
    if !origin && ctx.out.format == Format::Table {
        ctx.out.footer("where each value came from: --origin");
    }
    Ok(())
}

/// Never print a token, even a partial one.
fn mask(file: &mut ConfigFile) {
    for profile in file.profiles.values_mut() {
        if profile.token.is_some() {
            profile.token = Some("<redacted>".into());
        }
    }
    if file.server.onyx_api_key.is_some() {
        file.server.onyx_api_key = Some("<redacted>".into());
    }
    if file.server.api_token.is_some() {
        file.server.api_token = Some("<redacted>".into());
    }
    if let Some(dsn) = &file.server.database_url {
        file.server.database_url = Some(ovis_backend::config::redact_dsn(dsn));
    }
}

fn set(ctx: &Ctx, key: &str, value: &str) -> CliResult<()> {
    let mut file = ctx.cfg.file.clone();
    let parts: Vec<&str> = key.split('.').collect();

    match parts.as_slice() {
        ["default_profile"] => file.default_profile = Some(value.to_string()),

        ["profiles", name, field] => {
            let profile = file.profiles.entry((*name).to_string()).or_default();
            match *field {
                "server" => profile.server = Some(value.to_string()),
                "token" => profile.token = Some(value.to_string()),
                other => {
                    return usage(format!(
                        "unknown profile setting '{other}'; expected server or token"
                    ))
                }
            }
        }

        ["ui", field] => match *field {
            "color" => {
                // Validate now: a typo here would otherwise fail every later
                // command with a confusing message.
                value
                    .parse::<crate::output::ColorChoice>()
                    .map_err(CliError::Usage)?;
                file.ui.color = value.to_string();
            }
            "pager" => file.ui.pager = value.to_string(),
            "table_max_width" => {
                file.ui.table_max_width = value
                    .parse()
                    .map_err(|_| CliError::Usage(format!("'{value}' is not a width")))?
            }
            "hints" => {
                file.ui.hints = parse_bool(value)?;
            }
            other => return usage(format!("unknown ui setting '{other}'")),
        },

        ["tui", field] => match *field {
            "auto_refresh_secs" => {
                file.tui.auto_refresh_secs = value
                    .parse()
                    .map_err(|_| CliError::Usage(format!("'{value}' is not a number of seconds")))?
            }
            "default_screen" => match value {
                "pages" | "connectors" | "activity" => file.tui.default_screen = value.to_string(),
                other => {
                    return usage(format!(
                        "unknown screen '{other}'; expected pages, connectors or activity"
                    ))
                }
            },
            other => return usage(format!("unknown tui setting '{other}'")),
        },

        ["server", field] => {
            let slot = match *field {
                "host" => &mut file.server.host,
                "database_url" => &mut file.server.database_url,
                "opensearch_url" => &mut file.server.opensearch_url,
                "onyx_api_url" => &mut file.server.onyx_api_url,
                "onyx_api_key" => &mut file.server.onyx_api_key,
                "embed_api_url" => &mut file.server.embed_api_url,
                "api_token" => &mut file.server.api_token,
                "port" => {
                    file.server.port = Some(
                        value
                            .parse()
                            .map_err(|_| CliError::Usage(format!("'{value}' is not a port")))?,
                    );
                    return finish(ctx, &file, key, value);
                }
                other => return usage(format!("unknown server setting '{other}'")),
            };
            *slot = Some(value.to_string());
        }

        _ => {
            return usage(format!(
                "unknown key '{key}'. Try default_profile, profiles.<name>.server, \
                 profiles.<name>.token, ui.color, ui.pager, ui.hints, ui.table_max_width, \
                 tui.auto_refresh_secs, tui.default_screen, or server.<setting>"
            ))
        }
    }

    finish(ctx, &file, key, value)
}

fn parse_bool(value: &str) -> CliResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        other => usage(format!("'{other}' is not true or false")),
    }
}

fn finish(ctx: &Ctx, file: &ConfigFile, key: &str, value: &str) -> CliResult<()> {
    config::save_file(&ctx.cfg.path, file)?;
    // Values that could be secrets are acknowledged without being echoed —
    // shells keep history, and terminals keep scrollback.
    let secret = key.ends_with("token")
        || key.ends_with("key")
        || key.ends_with("password")
        || key == "server.database_url";
    let shown = if secret { "<set>" } else { value };
    ctx.out
        .print(format!("{key} = {shown}  ({})", ctx.cfg.path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;

    #[test]
    fn booleans_accept_the_usual_spellings() {
        for yes in ["true", "YES", "on", "1"] {
            assert!(parse_bool(yes).unwrap());
        }
        for no in ["false", "No", "off", "0"] {
            assert!(!parse_bool(no).unwrap());
        }
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn masking_removes_every_secret_shape_from_config_show() {
        let mut file = ConfigFile::default();
        file.profiles.insert(
            "homelab".into(),
            Profile {
                server: Some("http://gamma:8080".into()),
                token: Some("super-secret".into()),
            },
        );
        file.server.onyx_api_key = Some("onyx_pat_abc".into());
        file.server.api_token = Some("api-token".into());
        file.server.database_url = Some("postgres://postgres:hunter2@gamma:5433/postgres".into());

        mask(&mut file);
        let rendered = serde_json::to_string(&file).unwrap();
        assert!(!rendered.contains("super-secret"), "{rendered}");
        assert!(!rendered.contains("onyx_pat_abc"), "{rendered}");
        assert!(!rendered.contains("api-token"), "{rendered}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        // …but the non-secret parts survive, or the output would be useless.
        assert!(rendered.contains("http://gamma:8080"));
        assert!(rendered.contains("gamma:5433"));
    }
}
