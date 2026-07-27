use anyhow::Result;
use clap::Parser;
use ovis_cli::cli::{Cli, Commands, ConnectorCommands, PageCommands, PruneSubcommands, ServerSubcommands};
use ovis_cli::commands::{
    handle_connector_list, handle_page_delete, handle_page_edit, handle_page_inspect,
    handle_page_list, handle_prune_run, handle_server_restart, handle_server_start,
    handle_server_stop, handle_server_status, handle_tui,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Server {
            action,
            port,
            host,
            detach,
            auto_detect,
        } => match action {
            Some(ServerSubcommands::Start {
                port: p,
                host: h,
                detach: d,
                auto_detect: a,
            }) => {
                handle_server_start(&cli, *p, h, *d, *a).await?;
            }
            Some(ServerSubcommands::Stop { port: p }) => {
                handle_server_stop(*p).await?;
            }
            Some(ServerSubcommands::Restart {
                port: p,
                host: h,
                detach: d,
                auto_detect: a,
            }) => {
                handle_server_restart(&cli, *p, h, *d, *a).await?;
            }
            Some(ServerSubcommands::Status { port: p }) => {
                handle_server_status(*p).await?;
            }
            None => {
                handle_server_start(&cli, *port, host, *detach, *auto_detect).await?;
            }
        },
        Commands::Connector { action } => match action {
            ConnectorCommands::List => {
                handle_connector_list(&cli).await?;
            }
        },
        Commands::Page { action } => match action {
            PageCommands::List {
                connector,
                source,
                search,
                limit,
                offset,
            } => {
                handle_page_list(
                    &cli,
                    *connector,
                    source.clone(),
                    search.clone(),
                    *limit,
                    *offset,
                )
                .await?;
            }
            PageCommands::Inspect { id, raw } => {
                handle_page_inspect(&cli, id, *raw).await?;
            }
            PageCommands::Edit { id, title, tags } => {
                handle_page_edit(&cli, id, title.clone(), tags.clone()).await?;
            }
            PageCommands::Delete { id, yes } => {
                handle_page_delete(&cli, id, *yes).await?;
            }
        },
        Commands::Prune {
            action,
            config,
            dry_run,
            force,
        } => {
            let (cfg, dry, frc) = if let Some(PruneSubcommands::Run {
                config: c,
                dry_run: d,
                force: f,
            }) = action
            {
                (c.as_str(), *d, *f)
            } else {
                (config.as_str(), *dry_run, *force)
            };
            handle_prune_run(&cli, cfg, dry, frc).await?;
        }
        Commands::Tui { connector, search } => {
            handle_tui(&cli, *connector, search.clone()).await?;
        }
    }

    Ok(())
}
