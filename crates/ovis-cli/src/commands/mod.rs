//! Command dispatch.
//!
//! Note what is *not* here any more: `get_sample_documents`, `get_sample_chunks`,
//! `get_sample_connectors`, `get_sample_prune_docs` and `get_sample_inspect_doc`.
//! Every one of them turned a failure into plausible-looking fabricated output
//! with exit 0, including through `--format json`. There is no fallback data in
//! this crate; a failure is an error with a non-zero exit code.

pub mod completions;
pub mod config_cmd;
pub mod connector;
pub mod page;
pub mod prune;
pub mod search;
pub mod server;
pub mod stats;
pub mod status;

use crate::cli::{Cli, Command, ConnectorCommand, PageCommand, PruneCommand};
use crate::ctx::Ctx;
use crate::error::CliResult;

pub async fn dispatch(cli: &Cli) -> CliResult<()> {
    let ctx = Ctx::build(&cli.globals)?;

    match &cli.command {
        Command::Page { action } => page_command(&ctx, action).await,
        Command::Connector { action } => connector_command(&ctx, action).await,
        Command::Search(args) => search::run(&ctx, args).await,
        Command::Stats { action } => stats::run(&ctx, action.as_ref()).await,
        Command::Status => status::run(&ctx).await,
        Command::Tui(args) => crate::tui::run(&ctx, args).await,
        Command::Server { action } => server::run(&ctx, action).await,
        Command::Config { action } => config_cmd::run(&ctx, action),
        Command::Completions { shell } => completions::run(&ctx, *shell),
        Command::ConnectorNames => connector::names(&ctx).await,
        Command::Prune { action } => prune_command(&ctx, action).await,
    }
}

async fn prune_command(ctx: &Ctx, action: &PruneCommand) -> CliResult<()> {
    match action {
        PruneCommand::Scan(args) => prune::scan(ctx, args).await,
        PruneCommand::Scans { limit, page } => prune::scans(ctx, *limit, *page).await,
        PruneCommand::Ls(args) => prune::ls(ctx, args).await,
        PruneCommand::Show { id } => prune::show(ctx, id).await,
        PruneCommand::Dismiss { ids, forever } => prune::dismiss(ctx, ids, *forever).await,
        PruneCommand::Stage(args) => prune::stage(ctx, args).await,
        PruneCommand::Staged { limit, page } => prune::staged(ctx, *limit, *page).await,
        PruneCommand::Restore { ids, all_staged } => prune::restore(ctx, ids, *all_staged).await,
        PruneCommand::Delete(args) => prune::delete(ctx, args).await,
        PruneCommand::Status => prune::status(ctx).await,
        PruneCommand::Log {
            since,
            action,
            limit,
            page,
        } => prune::log(ctx, since.as_deref(), action.as_deref(), *limit, *page).await,
        PruneCommand::Rules { action } => prune::rules(ctx, action).await,
        PruneCommand::Config { action } => prune::config(ctx, action).await,
        PruneCommand::Exclusions { limit, page } => prune::exclusions(ctx, *limit, *page).await,
    }
}

async fn page_command(ctx: &Ctx, action: &PageCommand) -> CliResult<()> {
    match action {
        PageCommand::List(args) => page::list(ctx, args).await,
        PageCommand::View { id } => page::view(ctx, id).await,
        PageCommand::Text { id, output } => page::text(ctx, id, output.as_deref()).await,
        PageCommand::Chunks {
            id,
            limit,
            after,
            full,
        } => page::chunks(ctx, id, *limit, *after, *full).await,
        PageCommand::Open { id } => page::open(ctx, id).await,
        PageCommand::Edit {
            id,
            title,
            boost,
            hide,
            unhide,
            meta,
        } => page::edit(ctx, id, title.as_deref(), *boost, *hide, *unhide, meta).await,
        PageCommand::Delete { ids, from_file } => {
            page::delete(ctx, ids, from_file.as_deref()).await
        }
        PageCommand::Search(args) => search::run(ctx, args).await,
    }
}

async fn connector_command(ctx: &Ctx, action: &ConnectorCommand) -> CliResult<()> {
    match action {
        ConnectorCommand::List {
            query,
            status,
            parked,
            source,
            sort,
        } => {
            connector::list(
                ctx,
                &connector::ListFilters {
                    query: query.as_deref(),
                    status: status.as_deref(),
                    parked: *parked,
                    source: source.as_deref(),
                    sort,
                },
            )
            .await
        }
        ConnectorCommand::View { connector, history } => {
            connector::view(ctx, connector, history.as_deref()).await
        }
        ConnectorCommand::Docs {
            connector,
            limit,
            page,
        } => connector::docs(ctx, connector, *limit, *page).await,
        ConnectorCommand::Attempts {
            connector,
            status,
            limit,
            page,
        } => connector::attempts(ctx, connector.as_deref(), status.as_deref(), *limit, *page).await,
        ConnectorCommand::Errors {
            connector,
            limit,
            page,
            unresolved,
        } => connector::errors(ctx, connector, *limit, *page, *unresolved).await,
        ConnectorCommand::Pause { connectors } => connector::pause(ctx, connectors).await,
        ConnectorCommand::Resume { connectors } => connector::resume(ctx, connectors).await,
        ConnectorCommand::Run {
            connector,
            from_beginning,
            acknowledge_parked,
        } => connector::run(ctx, connector, *from_beginning, *acknowledge_parked).await,
        ConnectorCommand::Prune { connector } => connector::prune(ctx, connector).await,
        ConnectorCommand::Delete {
            connector,
            confirm_name,
        } => connector::delete(ctx, connector, confirm_name.as_deref()).await,
    }
}

#[cfg(test)]
mod tests {
    /// The audit's headline honesty defect, asserted as a property of the crate
    /// rather than of one call site: no fabricated data exists to fall back to.
    #[test]
    fn no_sample_data_generator_survives_anywhere_in_this_crate() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offenders = Vec::new();
        let mut stack = vec![root.join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src is readable") {
                let path = entry.expect("readable entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("readable source");
                    // This file names them in a comment, which is the point.
                    if path.file_name().is_some_and(|n| n == "mod.rs")
                        && path.parent().is_some_and(|p| p.ends_with("commands"))
                    {
                        continue;
                    }
                    if text.contains("fn get_sample_") {
                        offenders.push(path.display().to_string());
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "sample-data fallbacks are gone for good: {offenders:?}"
        );
    }
}
