//! `ovis stats` — deployment aggregates.

use crate::api::QueryBuilder;
use crate::cli::StatsCommand;
use crate::ctx::Ctx;
use crate::error::CliResult;
use crate::output::Format;
use crate::render;

pub async fn run(ctx: &Ctx, action: Option<&StatsCommand>) -> CliResult<()> {
    match action.unwrap_or(&StatsCommand::Overview) {
        StatsCommand::Overview => overview(ctx).await,
        StatsCommand::Connectors { limit, by } => top_connectors(ctx, *limit, by).await,
        StatsCommand::Timeline { window, bucket } => timeline(ctx, window, bucket.as_deref()).await,
        StatsCommand::Sources => sources(ctx).await,
    }
}

async fn overview(ctx: &Ctx) -> CliResult<()> {
    let stats = ctx.api.stats_overview().await?;
    match ctx.out.format {
        Format::Json => ctx.out.json(&stats),
        Format::Yaml => ctx.out.yaml(&stats),
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(&stats)),
        Format::Table | Format::Csv => {
            let grid = render::stats_overview(ctx, &stats);
            ctx.out.grid(&grid)
        }
    }
}

async fn sources(ctx: &Ctx) -> CliResult<()> {
    let sources = ctx.api.stats_sources().await?;
    match ctx.out.format {
        Format::Json => ctx.out.json(&sources),
        Format::Yaml => ctx.out.yaml(&sources),
        Format::Ndjson => ctx.out.ndjson(&sources),
        Format::Table | Format::Csv => {
            let grid = render::stats_sources(ctx, &sources);
            ctx.out.grid(&grid)
        }
    }
}

async fn top_connectors(ctx: &Ctx, limit: i64, by: &str) -> CliResult<()> {
    let mut query = QueryBuilder::new();
    query.push("limit", limit.max(1)).push("by", by);
    let items = ctx.api.stats_top_connectors(&query.build()).await?;

    match ctx.out.format {
        Format::Json => ctx.out.json(&items),
        Format::Yaml => ctx.out.yaml(&items),
        Format::Ndjson => ctx.out.ndjson(&items),
        Format::Table | Format::Csv => {
            let grid = render::stats_top_connectors(ctx, &items);
            ctx.out.grid(&grid)
        }
    }
}

async fn timeline(ctx: &Ctx, window: &str, bucket: Option<&str>) -> CliResult<()> {
    let mut query = QueryBuilder::new();
    query.push("window", window);
    query.push_opt("bucket", bucket);
    let response = ctx.api.stats_timeline(&query.build()).await?;

    match ctx.out.format {
        Format::Json => ctx.out.json(&response),
        Format::Yaml => ctx.out.yaml(&response),
        Format::Ndjson => ctx.out.ndjson(&response.items),
        Format::Table | Format::Csv => {
            let grid = render::stats_timeline(ctx, &response);
            ctx.out.grid(&grid)?;
            let total: i64 = response.items.iter().map(|b| b.docs).sum();
            ctx.out.footer(format!(
                "{} documents over {} ({} buckets of {})",
                crate::output::thousands(total),
                response.window,
                response.items.len(),
                response.bucket
            ));
            Ok(())
        }
    }
}
