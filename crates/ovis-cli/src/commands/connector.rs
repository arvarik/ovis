//! `ovis connector …` — the read side and the guarded actions.
//!
//! Two guardrails the backend enforces and this mirrors rather than bypasses:
//! a parked cc-pair refuses `run-once` without `acknowledge_parked`, and a
//! cc-pair delete requires the name typed back. Neither flag is ever set on the
//! user's behalf.

use ovis_core::api_types::{ActionResponse, ConnectorSummary, RunOnceRequest};

use crate::api::QueryBuilder;
use crate::ctx::Ctx;
use crate::error::{usage, CliError, CliResult};
use crate::handles::{self, HandleItem, HandleKind};
use crate::output::style::Tone;
use crate::output::{thousands, Format};
use crate::render;
use crate::resolve;

// ---------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------

pub struct ListFilters<'a> {
    pub query: Option<&'a str>,
    pub status: Option<&'a str>,
    pub parked: bool,
    pub source: Option<&'a str>,
    pub sort: &'a str,
}

pub async fn list(ctx: &Ctx, filters: &ListFilters<'_>) -> CliResult<()> {
    render::validate_columns(ctx, render::CONNECTOR_COLUMNS)?;
    let mut all = ctx.api.connectors().await?;
    let total = all.len();
    apply_filters(&mut all, filters)?;
    sort_connectors(&mut all, filters.sort)?;

    match ctx.out.format {
        // `GET /connectors` answers a bare array, and `--format json` is the API
        // response verbatim, so this is an array too.
        Format::Json => ctx.out.json(&all)?,
        Format::Yaml => ctx.out.yaml(&all)?,
        Format::Ndjson => ctx.out.ndjson(&all)?,
        Format::Table | Format::Csv => {
            let grid = render::connectors(ctx, &all, Some(1))?;
            ctx.out.grid(&grid)?;
        }
    }

    handles::save(
        HandleKind::Connector,
        "ovis connector list",
        all.iter()
            .enumerate()
            .map(|(offset, c)| HandleItem {
                n: offset + 1,
                id: c.cc_pair_id.to_string(),
                label: c.name.clone(),
            })
            .collect(),
    );

    if ctx.out.format == Format::Table {
        let docs: i64 = all.iter().map(|c| c.doc_count).sum();
        let parked = all.iter().filter(|c| c.parked).count();
        ctx.out.footer(format!(
            "{} of {total} connectors · {} documents{}",
            all.len(),
            thousands(docs),
            if parked > 0 {
                format!(" · {parked} parked")
            } else {
                String::new()
            }
        ));
        ctx.out
            .footer("view: ovis connector view @N · pages: ovis page list -c <name>");
    }
    Ok(())
}

fn apply_filters(all: &mut Vec<ConnectorSummary>, filters: &ListFilters<'_>) -> CliResult<()> {
    if let Some(query) = filters.query {
        let needle = query.to_ascii_lowercase();
        all.retain(|c| c.name.to_ascii_lowercase().contains(&needle));
    }
    if let Some(status) = filters.status {
        let wanted = status.to_ascii_uppercase();
        all.retain(|c| c.status.eq_ignore_ascii_case(&wanted));
    }
    if filters.parked {
        all.retain(|c| c.parked);
    }
    if let Some(source) = filters.source {
        let wanted = resolve::normalise_source(source);
        all.retain(|c| c.source.eq_ignore_ascii_case(&wanted));
    }
    Ok(())
}

fn sort_connectors(all: &mut [ConnectorSummary], sort: &str) -> CliResult<()> {
    match sort.to_ascii_lowercase().as_str() {
        "docs" => all.sort_by_key(|c| std::cmp::Reverse(c.doc_count)),
        "name" => all.sort_by_key(|a| a.name.to_lowercase()),
        "status" => all.sort_by(|a, b| a.status.cmp(&b.status).then(b.doc_count.cmp(&a.doc_count))),
        "source" => all.sort_by(|a, b| a.source.cmp(&b.source).then(b.doc_count.cmp(&a.doc_count))),
        other => {
            return usage(format!(
                "unknown sort '{other}'; expected name, docs, status or source"
            ))
        }
    }
    Ok(())
}

pub async fn view(ctx: &Ctx, reference: &str, history: Option<&str>) -> CliResult<()> {
    let resolved = resolve_reference(ctx, reference).await?;
    let mut query = QueryBuilder::new();
    query.push_opt("history", history);
    let detail = ctx.api.connector(resolved, &query.build()).await?;

    match ctx.out.format {
        Format::Json => ctx.out.json(&detail)?,
        Format::Yaml => ctx.out.yaml(&detail)?,
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(&detail))?,
        Format::Table | Format::Csv => {
            let grid = render::connector_detail(ctx, &detail);
            ctx.out.grid(&grid)?;
        }
    }

    if ctx.out.format == Format::Table {
        ctx.out.footer(format!(
            "pages: ovis page list -c {name} · attempts: ovis connector attempts {name} · \
             errors: ovis connector errors {name}",
            name = detail.summary.name
        ));
    }
    Ok(())
}

pub async fn docs(
    ctx: &Ctx,
    reference: &str,
    limit: Option<i64>,
    page: Option<i64>,
) -> CliResult<()> {
    render::validate_columns(ctx, render::PAGE_COLUMNS)?;
    let cc_pair_id = resolve_reference(ctx, reference).await?;
    let limit = limit.unwrap_or_else(|| ctx.out.default_limit()).max(1);
    let page = page.unwrap_or(1).max(1);

    let mut query = QueryBuilder::new();
    query.push("limit", limit).push("page", page);
    let response = ctx.api.connector_docs(cc_pair_id, &query.build()).await?;

    let first_handle = ((page - 1) * limit + 1) as usize;
    match ctx.out.format {
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let grid = render::pages(ctx, &response.items, Some(first_handle))?;
            ctx.out.grid(&grid)?;
        }
    }

    handles::save(
        HandleKind::Page,
        &format!("ovis connector docs {reference}"),
        response
            .items
            .iter()
            .enumerate()
            .map(|(offset, item)| HandleItem {
                n: first_handle + offset,
                id: item.id.clone(),
                label: item.semantic_id.clone(),
            })
            .collect(),
    );

    if ctx.out.format == Format::Table {
        ctx.out.footer(format!(
            "{}–{} of {}{}",
            first_handle,
            first_handle + response.items.len().saturating_sub(1),
            thousands(response.total),
            if response.has_more {
                format!(
                    " · next: ovis connector docs {reference} --page {}",
                    page + 1
                )
            } else {
                String::new()
            }
        ));
    }
    Ok(())
}

pub async fn attempts(
    ctx: &Ctx,
    reference: Option<&str>,
    status: Option<&str>,
    limit: Option<i64>,
    page: Option<i64>,
) -> CliResult<()> {
    render::validate_columns(ctx, render::ATTEMPT_COLUMNS)?;
    let limit = limit.unwrap_or_else(|| ctx.out.default_limit()).max(1);
    let page = page.unwrap_or(1).max(1);
    let mut query = QueryBuilder::new();
    query.push("limit", limit).push("page", page);
    query.push_opt("status", status);

    // Scoped to one connector, or global — both are useful, and the global view
    // is what the Activity screen shows.
    let response = match reference {
        Some(reference) => {
            let cc_pair_id = resolve_reference(ctx, reference).await?;
            ctx.api
                .connector_attempts(cc_pair_id, &query.build())
                .await?
        }
        None => ctx.api.attempts(&query.build()).await?,
    };

    match ctx.out.format {
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let grid = render::attempts(ctx, &response.items)?;
            ctx.out.grid(&grid)?;
        }
    }

    if ctx.out.format == Format::Table {
        let stalled = response.items.iter().filter(|a| a.stalled).count();
        ctx.out.footer(format!(
            "{} of {} attempts{}",
            response.items.len(),
            thousands(response.total),
            if stalled > 0 {
                format!(" · {stalled} STALLED (no heartbeat for 45 min)")
            } else {
                String::new()
            }
        ));
    }
    Ok(())
}

pub async fn errors(
    ctx: &Ctx,
    reference: &str,
    limit: Option<i64>,
    page: Option<i64>,
    unresolved: bool,
) -> CliResult<()> {
    render::validate_columns(ctx, render::ATTEMPT_ERROR_COLUMNS)?;
    let cc_pair_id = resolve_reference(ctx, reference).await?;
    let limit = limit.unwrap_or_else(|| ctx.out.default_limit()).max(1);
    let page = page.unwrap_or(1).max(1);

    let mut query = QueryBuilder::new();
    query.push("limit", limit).push("page", page);
    if unresolved {
        query.push("unresolved_only", true);
    }
    let response = ctx.api.connector_errors(cc_pair_id, &query.build()).await?;

    let first_handle = ((page - 1) * limit + 1) as usize;
    match ctx.out.format {
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let grid = render::attempt_errors(ctx, &response.items, Some(first_handle))?;
            ctx.out.grid(&grid)?;
        }
    }

    // A failing URL should be one `ovis page view @4` away.
    handles::save(
        HandleKind::Page,
        &format!("ovis connector errors {reference}"),
        response
            .items
            .iter()
            .enumerate()
            .filter_map(|(offset, e)| {
                e.document_id.as_ref().map(|id| HandleItem {
                    n: first_handle + offset,
                    id: id.clone(),
                    label: e.failure_message.clone(),
                })
            })
            .collect(),
    );

    if ctx.out.format == Format::Table {
        // The rolling window matters: an empty list is not "nothing ever failed".
        ctx.out.footer(format!(
            "{} of {} errors · window: {} (the resilience cron prunes older rows)",
            response.items.len(),
            thousands(response.total),
            response.window
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// actions
// ---------------------------------------------------------------------------

async fn resolve_reference(ctx: &Ctx, reference: &str) -> CliResult<i32> {
    // A `@N` from `connector list` carries the cc-pair id directly.
    if handles::is_handle(reference) {
        let raw = handles::resolve(reference, HandleKind::Connector)?;
        return raw
            .parse::<i32>()
            .map_err(|_| CliError::StaleHandle(format!("{reference} does not hold a cc-pair id")));
    }
    Ok(resolve::connector(ctx, reference).await?.cc_pair_id())
}

pub async fn pause(ctx: &Ctx, references: &[String]) -> CliResult<()> {
    act_on_many(ctx, references, "pause").await
}

pub async fn resume(ctx: &Ctx, references: &[String]) -> CliResult<()> {
    act_on_many(ctx, references, "resume").await
}

pub async fn prune(ctx: &Ctx, reference: &str) -> CliResult<()> {
    let resolved = resolve::connector(ctx, reference).await?;
    let response = ctx
        .api
        .connector_action(resolved.cc_pair_id(), "prune")
        .await?;
    report_action(ctx, resolved.name(), &response)
}

async fn act_on_many(ctx: &Ctx, references: &[String], action: &str) -> CliResult<()> {
    // One target is not a batch: a typo should exit 3 (not found), not 11
    // (partial failure of nothing).
    let single = references.len() == 1;
    let mut outcomes = Vec::new();
    let mut failures = Vec::new();

    for reference in references {
        let resolved = match resolve::connector(ctx, reference).await {
            Ok(resolved) => resolved,
            Err(err) if single => return Err(err),
            Err(err) => {
                failures.push(format!("{reference}: {}", err.message()));
                continue;
            }
        };
        match ctx
            .api
            .connector_action(resolved.cc_pair_id(), action)
            .await
        {
            Ok(response) => {
                if ctx.out.format == Format::Table {
                    report_action(ctx, resolved.name(), &response)?;
                }
                outcomes.push(response);
            }
            Err(err) if single => return Err(err),
            Err(err) => failures.push(format!("{}: {}", resolved.name(), err.message())),
        }
    }

    match ctx.out.format {
        Format::Json => ctx.out.json(&outcomes)?,
        Format::Yaml => ctx.out.yaml(&outcomes)?,
        Format::Ndjson => ctx.out.ndjson(&outcomes)?,
        _ => {}
    }

    if !failures.is_empty() {
        for failure in &failures {
            ctx.out.warn(failure);
        }
        // Some worked and some did not; the exit code has to say so.
        return Err(CliError::PartialFailure(format!(
            "{action}d {}, failed {}",
            outcomes.len(),
            failures.len()
        )));
    }
    Ok(())
}

pub async fn run(
    ctx: &Ctx,
    reference: &str,
    from_beginning: bool,
    acknowledge_parked: bool,
) -> CliResult<()> {
    let resolved = resolve::connector(ctx, reference).await?;
    let summary = &resolved.summary;

    // Never pass acknowledge_parked on the user's behalf. A park sentinel means
    // the resilience cron deliberately finished with this cc-pair; overriding
    // that is a decision, not a detail.
    let mut acknowledged = acknowledge_parked;
    if summary.parked && !acknowledged {
        let sentinel = summary
            .last_attempt
            .as_ref()
            .and_then(|a| a.error_msg.as_deref())
            .unwrap_or("a park sentinel");
        ctx.out.warn(format!(
            "{} is parked: its last attempt carries {}. The homelab resilience cron skips \
             parked cc-pairs deliberately — a first-pass crawl is already complete.",
            summary.name,
            sentinel.trim()
        ));
        if !crate::prompt::confirm(&format!("crawl {} anyway?", summary.name), ctx.interaction)? {
            return Err(CliError::NeedsConfirmation(
                "cancelled; no crawl was triggered".into(),
            ));
        }
        acknowledged = true;
    }

    if from_beginning && summary.doc_count > 10_000 {
        ctx.out.warn(format!(
            "--from-beginning re-crawls all {} documents of {}",
            thousands(summary.doc_count),
            summary.name
        ));
        if !crate::prompt::confirm("proceed?", ctx.interaction)? {
            return Err(CliError::NeedsConfirmation(
                "cancelled; no crawl was triggered".into(),
            ));
        }
    }

    let response = ctx
        .api
        .connector_run_once(
            resolved.cc_pair_id(),
            &RunOnceRequest {
                from_beginning,
                acknowledge_parked: acknowledged,
            },
        )
        .await?;
    report_action(ctx, resolved.name(), &response)
}

pub async fn delete(ctx: &Ctx, reference: &str, confirm_name: Option<&str>) -> CliResult<()> {
    let resolved = resolve::connector(ctx, reference).await?;
    let summary = &resolved.summary;

    ctx.out.warn(format!(
        "deleting cc-pair {} ({}) removes the connector and all {} of its indexed documents",
        summary.cc_pair_id,
        summary.name,
        thousands(summary.doc_count)
    ));

    // The name echo is required even with -y: this can destroy 100k documents.
    match confirm_name {
        Some(given) if given == summary.name => {}
        Some(given) => {
            return Err(CliError::NeedsConfirmation(format!(
                "--confirm-name '{given}' does not match '{}'; nothing was changed",
                summary.name
            )))
        }
        None => crate::prompt::confirm_exact(
            &format!("this will delete '{}' and its documents.", summary.name),
            &summary.name,
            ctx.interaction,
        )?,
    }

    let response = ctx
        .api
        .connector_delete(resolved.cc_pair_id(), &summary.name)
        .await?;
    report_action(ctx, resolved.name(), &response)
}

fn report_action(ctx: &Ctx, name: &str, response: &ActionResponse) -> CliResult<()> {
    match ctx.out.format {
        Format::Json => return ctx.out.json(response),
        Format::Yaml => return ctx.out.yaml(response),
        Format::Ndjson => return ctx.out.ndjson(std::slice::from_ref(response)),
        _ => {}
    }

    let tone = if response.ok { Tone::Ok } else { Tone::Error };
    let mark = Tone::paint(
        tone,
        if response.ok { "ok" } else { "failed" },
        ctx.out.color,
    );
    let mut line = format!(
        "{mark}  {} {} (cc-pair {})",
        response.action, name, response.cc_pair_id
    );
    if let Some(status) = &response.status {
        line.push_str(&format!(" → {status}"));
    }
    ctx.out.print(line)?;

    // Onyx's own words, when it sent any — not paraphrased.
    if let Some(detail) = &response.detail {
        if !detail.trim().is_empty() {
            ctx.out.note(format!("onyx: {}", detail.trim()));
        }
    }
    Ok(())
}

/// The hidden command shell completions call to complete connector names.
pub async fn names(ctx: &Ctx) -> CliResult<()> {
    let all = ctx.api.connectors().await?;
    let mut buf = String::new();
    for c in all {
        buf.push_str(&c.name);
        buf.push('\n');
    }
    ctx.out.print(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        name: &str,
        source: &str,
        status: &str,
        docs: i64,
        parked: bool,
    ) -> ConnectorSummary {
        ConnectorSummary {
            connector_id: 1,
            cc_pair_id: 1,
            name: name.into(),
            source: source.into(),
            status: status.into(),
            parked,
            in_repeated_error_state: false,
            doc_count: docs,
            last_successful_index_time: None,
            refresh_freq_secs: None,
            indexing_trigger: None,
            last_attempt: None,
        }
    }

    fn fleet() -> Vec<ConnectorSummary> {
        vec![
            summary("tildes", "WEB", "PAUSED", 105_666, false),
            summary("jax-docs", "WEB", "ACTIVE", 4_200, false),
            summary("onyx-repo", "GITHUB", "ACTIVE", 1_743, true),
            summary("wikiquote", "WIKIPEDIA", "PAUSED", 0, false),
        ]
    }

    fn filters<'a>() -> ListFilters<'a> {
        ListFilters {
            query: None,
            status: None,
            parked: false,
            source: None,
            sort: "docs",
        }
    }

    #[test]
    fn the_default_sort_is_by_document_count_descending() {
        let mut all = fleet();
        sort_connectors(&mut all, "docs").unwrap();
        assert_eq!(all[0].name, "tildes");
        assert_eq!(all[3].name, "wikiquote");
    }

    #[test]
    fn sorting_by_name_is_case_insensitive() {
        let mut all = fleet();
        sort_connectors(&mut all, "name").unwrap();
        let names: Vec<&str> = all.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["jax-docs", "onyx-repo", "tildes", "wikiquote"]);
    }

    #[test]
    fn an_unknown_sort_is_a_usage_error_naming_the_options() {
        let mut all = fleet();
        let err = sort_connectors(&mut all, "chunks").unwrap_err();
        assert_eq!(err.exit_code(), crate::error::exit::USAGE);
        assert!(err.message().contains("docs"), "{}", err.message());
    }

    #[test]
    fn filters_narrow_by_name_status_source_and_park_state() {
        let mut all = fleet();
        apply_filters(
            &mut all,
            &ListFilters {
                query: Some("doc"),
                ..filters()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "jax-docs");

        let mut all = fleet();
        apply_filters(
            &mut all,
            &ListFilters {
                status: Some("active"),
                ..filters()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 2);

        let mut all = fleet();
        apply_filters(
            &mut all,
            &ListFilters {
                source: Some("github"),
                ..filters()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 1);

        let mut all = fleet();
        apply_filters(
            &mut all,
            &ListFilters {
                parked: true,
                ..filters()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "onyx-repo");
    }

    #[test]
    fn filters_compose_rather_than_overriding_one_another() {
        let mut all = fleet();
        apply_filters(
            &mut all,
            &ListFilters {
                source: Some("web"),
                status: Some("ACTIVE"),
                ..filters()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "jax-docs");
    }
}
