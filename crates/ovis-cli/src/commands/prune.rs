//! `ovis prune …` — review-first pruning.
//!
//! The contracts carried over from the CLI's constitution, plus this track's
//! own: `stage`/`delete` prompt with the count, chunk sum and risk breakdown;
//! `--no-input` turns any prompt into exit 10; bulk beyond the server's
//! `big_batch` needs `--confirm-count N` matching exactly, and **`-y` does not
//! waive that**; exit 11 on partial failure; exit 13 when the reaper is
//! halted; `delete` schedules — the reaper executes after the grace period,
//! and `--now` does not exist.

use ovis_core::api_types::{
    PruneBulkResponse, PruneCandidateFilterBody, PruneCandidateItem,
    PruneDismissRequest, PruneReason, PruneRestoreRequest, PruneRuleCreate, PruneRuleItem,
    PruneRulePatch, PruneScanItem, PruneScanRequest, PruneScheduleDeleteRequest, PruneScope,
    PruneStageRequest, PruneStatusResponse,
};

use crate::api::QueryBuilder;
use crate::cli::{
    PruneConfigCommand, PruneDeleteArgs, PruneLsArgs, PruneRulesCommand, PruneScanArgs,
    PruneSelectorArgs,
};
use crate::ctx::Ctx;
use crate::error::{usage, CliError, CliResult};
use crate::handles::{self, HandleItem, HandleKind};
use crate::output::style::Tone;
use crate::output::table::{Grid, GridCell};
use crate::output::{thousands, timestamp, timestamp_opt, Format};
use crate::prompt;
use crate::resolve;

// ---------------------------------------------------------------------------
// scan
// ---------------------------------------------------------------------------

pub async fn scan(ctx: &Ctx, args: &PruneScanArgs) -> CliResult<()> {
    let scope = if !args.connectors.is_empty() {
        let mut connector_ids = Vec::new();
        for reference in &args.connectors {
            let resolved = resolve::connector(ctx, reference).await?;
            connector_ids.push(resolved.connector_id());
        }
        PruneScope {
            kind: "connectors".into(),
            connector_ids: Some(connector_ids),
            url_prefix: None,
        }
    } else if let Some(prefix) = &args.prefix {
        PruneScope {
            kind: "url_prefix".into(),
            connector_ids: None,
            url_prefix: Some(prefix.clone()),
        }
    } else if args.all {
        PruneScope {
            kind: "all".into(),
            connector_ids: None,
            url_prefix: None,
        }
    } else {
        return usage(
            "say what to scan: --all, one or more -c CONNECTOR, or --prefix URL. A full-corpus \
             scan is fine — it is a preview and changes nothing",
        );
    };

    let scan = ctx
        .api
        .prune_scan_create(&PruneScanRequest {
            scope,
            detectors: args.detectors.clone(),
            config_overrides: None,
        })
        .await?;

    ctx.out.note(format!(
        "scan {} queued ({}) — a scan is a preview; nothing is hidden or deleted",
        scan.id,
        args.detectors.join(", ")
    ));

    if args.no_follow {
        if ctx.out.format != Format::Table {
            emit_scan(ctx, &scan)?;
        } else {
            ctx.out
                .footer(format!("follow: ovis prune scans · results: ovis prune ls --scan {}", scan.id));
        }
        return Ok(());
    }

    let finished = follow_scan(ctx, scan.id).await?;
    emit_scan(ctx, &finished)?;
    if finished.status == "failed" {
        return Err(CliError::Other(anyhow::anyhow!(
            "scan {} failed: {}",
            finished.id,
            finished.error.as_deref().unwrap_or("unknown error")
        )));
    }
    if ctx.out.format == Format::Table {
        ctx.out.footer(format!(
            "review: ovis prune ls --scan {} · evidence: ovis prune show @N",
            finished.id
        ));
    }
    Ok(())
}

async fn follow_scan(ctx: &Ctx, id: i64) -> CliResult<PruneScanItem> {
    let mut last_line = String::new();
    loop {
        let scan = ctx.api.prune_scan(id).await?;
        match scan.status.as_str() {
            "queued" | "running" => {
                let line = match scan.total {
                    Some(total) if total > 0 => format!(
                        "scanning… {} / {} documents",
                        thousands(scan.examined),
                        thousands(total)
                    ),
                    _ => format!("scanning… {} documents", thousands(scan.examined)),
                };
                if line != last_line {
                    ctx.out.note(&line);
                    last_line = line;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
            _ => return Ok(scan),
        }
    }
}

fn emit_scan(ctx: &Ctx, scan: &PruneScanItem) -> CliResult<()> {
    match ctx.out.format {
        Format::Json => ctx.out.json(scan),
        Format::Yaml => ctx.out.yaml(scan),
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(scan)),
        Format::Table | Format::Csv => {
            let mut grid = Grid::new(vec!["field".into(), "value".into()]);
            let stats = &scan.stats;
            let stat = |key: &str| stats.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
            grid.push(vec![
                GridCell::plain("scan"),
                GridCell::plain(scan.id.to_string()),
            ]);
            grid.push(vec![
                GridCell::plain("status"),
                GridCell::toned(&scan.status, crate::output::style::status_tone(&scan.status)),
            ]);
            grid.push(vec![
                GridCell::plain("examined"),
                GridCell::plain(format!(
                    "{}{}",
                    thousands(scan.examined),
                    scan.total
                        .map(|t| format!(" of {}", thousands(t)))
                        .unwrap_or_default()
                )),
            ]);
            grid.push(vec![
                GridCell::plain("new candidates"),
                GridCell::plain(thousands(stat("candidates_new"))),
            ]);
            grid.push(vec![
                GridCell::plain("updated"),
                GridCell::plain(thousands(stat("candidates_updated"))),
            ]);
            grid.push(vec![
                GridCell::plain("closed (no longer match)"),
                GridCell::plain(thousands(stat("candidates_closed"))),
            ]);
            grid.push(vec![
                GridCell::plain("skipped (excluded)"),
                GridCell::plain(thousands(stat("excluded_skipped"))),
            ]);
            if let Some(error) = &scan.error {
                grid.push(vec![
                    GridCell::plain("error"),
                    GridCell::toned(error, Tone::Error),
                ]);
            }
            ctx.out.grid(&grid)
        }
    }
}

pub async fn scans(ctx: &Ctx, limit: Option<i64>, page: Option<i64>) -> CliResult<()> {
    let mut query = QueryBuilder::new();
    query.push_opt("limit", limit);
    query.push_opt("page", page);
    let response = ctx.api.prune_scans(&query.build()).await?;

    match ctx.out.format {
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let relative = ctx.relative_time();
            let mut grid = Grid::new(
                ["id", "status", "scope", "detectors", "examined", "found", "started"]
                    .map(String::from)
                    .to_vec(),
            );
            for scan in &response.items {
                let found = scan
                    .stats
                    .get("candidates_new")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                grid.push(vec![
                    GridCell::plain(scan.id.to_string()),
                    GridCell::toned(&scan.status, crate::output::style::status_tone(&scan.status)),
                    GridCell::plain(describe_scope(&scan.scope)),
                    GridCell::plain(scan.detectors.join(",")),
                    GridCell::plain(thousands(scan.examined)),
                    GridCell::plain(thousands(found)),
                    GridCell::plain(timestamp_opt(scan.started_at.as_ref(), relative)),
                ]);
            }
            ctx.out.grid(&grid)?;
            ctx.out.footer(format!("{} scans", thousands(response.total)));
        }
    }
    Ok(())
}

fn describe_scope(scope: &PruneScope) -> String {
    match scope.kind.as_str() {
        "connectors" => format!(
            "connectors {}",
            scope
                .connector_ids
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        "url_prefix" => format!("prefix {}", scope.url_prefix.as_deref().unwrap_or("")),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// ls / staged / show
// ---------------------------------------------------------------------------

pub async fn ls(ctx: &Ctx, args: &PruneLsArgs) -> CliResult<()> {
    let mut query = QueryBuilder::new();
    query.push_opt("detector", args.detector.as_deref());
    query.push_opt("state", args.state.as_deref());
    query.push_opt("min_confidence", args.min_confidence);
    if args.risky {
        query.push("recrawl_risk", true);
    }
    if let Some(reference) = &args.connector {
        let resolved = resolve::connector(ctx, reference).await?;
        query.push("connector_id", resolved.connector_id());
    }
    query.push_opt("scan_id", args.scan);
    if let Some(sort) = &args.sort {
        query.push("sort", parse_prune_sort(sort)?);
    }
    let limit = args.limit.unwrap_or_else(|| ctx.out.default_limit()).max(1);
    query.push("limit", limit);
    let page = args.page.unwrap_or(1).max(1);
    query.push("page", page);

    let response = ctx.api.prune_candidates(&query.build()).await?;
    let first_handle = ((page - 1) * limit + 1) as usize;
    emit_candidates(ctx, &response.items, first_handle, "ovis prune ls")?;

    match ctx.out.format {
        Format::Table | Format::Csv => {
            if response.items.is_empty() {
                ctx.out.footer(
                    "no candidates matched · run a scan first: ovis prune scan -c NAME -d thin \
                     -d exact_duplicate",
                );
            } else {
                let last = first_handle as i64 + response.items.len() as i64 - 1;
                let mut footer = format!(
                    "{}–{} of {}",
                    thousands(first_handle as i64),
                    thousands(last),
                    thousands(response.total)
                );
                if response.has_more {
                    footer.push_str(&format!(" · next: ovis prune ls --page {}", page + 1));
                }
                ctx.out.footer(footer);
                ctx.out.footer(
                    "evidence: ovis prune show @N · stage: ovis prune stage @N · dismiss: \
                     ovis prune dismiss @N",
                );
            }
        }
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => {}
    }
    Ok(())
}

pub async fn staged(ctx: &Ctx, limit: Option<i64>, page: Option<i64>) -> CliResult<()> {
    let mut query = QueryBuilder::new();
    query.push("state", "staged");
    query.push("sort", "expiry_asc");
    let limit = limit.unwrap_or_else(|| ctx.out.default_limit()).max(1);
    query.push("limit", limit);
    let page = page.unwrap_or(1).max(1);
    query.push("page", page);

    let response = ctx.api.prune_candidates(&query.build()).await?;
    let first_handle = ((page - 1) * limit + 1) as usize;
    emit_candidates(ctx, &response.items, first_handle, "ovis prune staged")?;

    if matches!(ctx.out.format, Format::Table | Format::Csv) {
        if response.items.is_empty() {
            ctx.out.footer("nothing is staged");
        } else {
            ctx.out.footer(format!(
                "{} staged · hidden from Onyx search but fully intact; each deletes \
                 automatically when its grace ends",
                thousands(response.total)
            ));
            ctx.out
                .footer("restore: ovis prune restore @N · sooner: ovis prune delete @N");
        }
    } else if ctx.out.format == Format::Json {
        // Already emitted above for json/yaml.
    }
    Ok(())
}

fn parse_prune_sort(raw: &str) -> CliResult<String> {
    let (field, direction) = match raw.split_once(':') {
        Some((f, d)) => (f, Some(d)),
        None => (raw, None),
    };
    let sorted = match (field, direction) {
        ("confidence", None | Some("desc")) => "confidence_desc",
        ("chunks", None | Some("desc")) => "chunks_desc",
        ("chunks", Some("asc")) => "chunks_asc",
        ("created", None | Some("desc")) => "created_desc",
        ("created", Some("asc")) => "created_asc",
        ("expiry", _) => "expiry_asc",
        _ => {
            return usage(format!(
                "unknown sort '{raw}'; expected confidence, chunks, created or expiry, \
                 optionally :asc/:desc"
            ))
        }
    };
    Ok(sorted.to_string())
}

fn reason_chips(reasons: &[PruneReason]) -> String {
    reasons
        .iter()
        .map(|r| match r.detector.as_str() {
            "duplicate" => format!("dup {:.0}%", r.confidence * 100.0),
            "language" => format!(
                "lang {} {:.2}",
                r.evidence
                    .get("detected")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?"),
                r.confidence
            ),
            "url_rule" | "tag_rule" => format!("rule {}", r.code),
            "thin" if r.code == "chunkless_stub" => "stub".to_string(),
            "thin" => format!("thin {:.1}", r.confidence),
            "stale" => "stale".to_string(),
            "recrawl" => "recrawled".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn emit_candidates(
    ctx: &Ctx,
    items: &[PruneCandidateItem],
    first_handle: usize,
    command: &str,
) -> CliResult<()> {
    match ctx.out.format {
        Format::Json | Format::Yaml => {} // caller emits the whole envelope
        Format::Ndjson => ctx.out.ndjson(items)?,
        Format::Table | Format::Csv => {
            let relative = ctx.relative_time();
            let staged_view = items.iter().any(|i| i.state == "staged");
            let mut headers = vec![
                "#".to_string(),
                "document".to_string(),
                "connector".to_string(),
                "reasons".to_string(),
                "conf".to_string(),
                "chunks".to_string(),
                "risk".to_string(),
            ];
            if staged_view {
                headers.push("grace ends".to_string());
            } else {
                headers.push("state".to_string());
            }
            let mut grid = Grid::new(headers);
            for (offset, item) in items.iter().enumerate() {
                let label = item
                    .link
                    .as_deref()
                    .or(item.semantic_id.as_deref())
                    .unwrap_or(&item.document_id);
                let mut row = vec![
                    GridCell::toned(format!("@{}", first_handle + offset), Tone::Dim),
                    GridCell::plain(label),
                    GridCell::plain(item.connector_name.as_deref().unwrap_or("—")),
                    GridCell::plain(reason_chips(&item.reasons)),
                    GridCell::plain(format!("{:.2}", item.confidence)),
                    match item.chunk_count {
                        Some(n) => GridCell::plain(n.to_string()),
                        None => GridCell::toned("—", Tone::Dim),
                    },
                    if item.recrawl_risk {
                        GridCell::toned("recrawl", Tone::Warn)
                    } else {
                        GridCell::toned("no", Tone::Dim)
                    },
                ];
                if staged_view {
                    row.push(GridCell::plain(timestamp_opt(
                        item.stage_expires_at.as_ref(),
                        relative,
                    )));
                } else {
                    row.push(GridCell::plain(&item.state));
                }
                grid.push(row);
            }
            ctx.out.grid(&grid)?;
        }
    }

    handles::save(
        HandleKind::PruneCandidate,
        command,
        items
            .iter()
            .enumerate()
            .map(|(offset, item)| HandleItem {
                n: first_handle + offset,
                id: item.id.to_string(),
                label: item.document_id.clone(),
            })
            .collect(),
    );
    Ok(())
}

pub async fn show(ctx: &Ctx, reference: &str) -> CliResult<()> {
    let id = resolve_candidate_id(reference)?;
    let detail = ctx.api.prune_candidate(id).await?;

    match ctx.out.format {
        Format::Json => return ctx.out.json(&detail),
        Format::Yaml => return ctx.out.yaml(&detail),
        Format::Ndjson => return ctx.out.ndjson(std::slice::from_ref(&detail)),
        Format::Table | Format::Csv => {}
    }

    let item = &detail.item;
    let relative = ctx.relative_time();
    let mut grid = Grid::new(vec!["field".into(), "value".into()]);
    let mut push = |k: &str, cell: GridCell| grid.push(vec![GridCell::plain(k), cell]);

    push("candidate", GridCell::plain(format!("#{}", item.id)));
    push("document", GridCell::plain(&item.document_id));
    if let Some(title) = &item.semantic_id {
        push("title", GridCell::plain(title));
    }
    push(
        "state",
        GridCell::toned(&item.state, crate::output::style::status_tone(&item.state)),
    );
    push("confidence", GridCell::plain(format!("{:.2}", item.confidence)));
    push(
        "connector",
        GridCell::plain(item.connector_name.as_deref().unwrap_or("—")),
    );
    match item.chunk_count {
        Some(n) => push("chunks", GridCell::plain(n.to_string())),
        None => push("chunks", GridCell::toned("not counted yet", Tone::Dim)),
    }
    push(
        "recrawl risk",
        if item.recrawl_risk {
            GridCell::toned(
                "yes — an active connector will likely crawl this back after deletion",
                Tone::Warn,
            )
        } else {
            GridCell::plain("no")
        },
    );
    if !item.doc_exists {
        push("document row", GridCell::toned("gone", Tone::Error));
    }
    if let Some(hidden) = item.hidden {
        push("hidden now", GridCell::plain(if hidden { "yes" } else { "no" }));
    }
    if let Some(staged_at) = &item.staged_at {
        push("staged", GridCell::plain(timestamp(staged_at, relative)));
        push(
            "grace ends",
            GridCell::toned(
                timestamp_opt(item.stage_expires_at.as_ref(), relative),
                Tone::Warn,
            ),
        );
        push(
            "was hidden before staging",
            GridCell::plain(match item.prev_hidden {
                Some(true) => "yes (restore returns it to hidden)",
                _ => "no",
            }),
        );
    }
    if detail.excluded {
        push(
            "exclusion list",
            GridCell::plain("yes — scans never re-flag this document"),
        );
    }

    for (index, reason) in item.reasons.iter().enumerate() {
        push(
            &format!("reason {}", index + 1),
            GridCell::plain(format!(
                "[{}/{}] {} (confidence {:.2})",
                reason.detector, reason.code, reason.detail, reason.confidence
            )),
        );
    }

    if let Some(pair) = &detail.pair {
        push("", GridCell::plain(""));
        push(
            "duplicate of",
            GridCell::toned(&pair.kept_id, Tone::Bold),
        );
        push("similarity", GridCell::plain(format!("{:.1}%", pair.similarity * 100.0)));
        match &pair.kept {
            Some(kept) => {
                push("keeper title", GridCell::plain(&kept.semantic_id));
                push(
                    "keeper chunks",
                    match kept.chunk_count {
                        Some(n) => GridCell::plain(n.to_string()),
                        None => GridCell::toned("—", Tone::Dim),
                    },
                );
                push(
                    "keeper updated",
                    GridCell::plain(timestamp(&kept.updated_at, relative)),
                );
            }
            None => push("keeper", GridCell::toned("no longer exists", Tone::Warn)),
        }
    }

    ctx.out.grid(&grid)?;

    if ctx.out.format == Format::Table && ctx.out.stdout_tty {
        if let Some(pair) = &detail.pair {
            ctx.out.footer(format!(
                "compare: ovis page text '{}' | less · ovis page text '{}' | less",
                item.document_id, pair.kept_id
            ));
        }
        match item.state.as_str() {
            "candidate" => ctx.out.footer(
                "stage: ovis prune stage @N · dismiss: ovis prune dismiss @N [--forever]",
            ),
            "staged" => ctx
                .out
                .footer("restore: ovis prune restore @N · sooner: ovis prune delete @N"),
            _ => {}
        }
    }
    Ok(())
}

fn resolve_candidate_id(reference: &str) -> CliResult<i64> {
    let resolved = handles::resolve(reference, HandleKind::PruneCandidate)?;
    resolved.parse::<i64>().map_err(|_| {
        CliError::Usage(format!(
            "'{reference}' is not a candidate id or @N handle from `ovis prune ls`"
        ))
    })
}

fn resolve_candidate_ids(references: &[String]) -> CliResult<Vec<i64>> {
    let resolved = handles::resolve_all(references, HandleKind::PruneCandidate)?;
    let mut ids = Vec::with_capacity(resolved.len());
    for raw in &resolved {
        ids.push(raw.parse::<i64>().map_err(|_| {
            CliError::Usage(format!(
                "'{raw}' is not a candidate id or @N handle from `ovis prune ls`"
            ))
        })?);
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Selection plumbing shared by stage/delete
// ---------------------------------------------------------------------------

struct Selection {
    ids: Option<Vec<i64>>,
    filter: Option<PruneCandidateFilterBody>,
    /// The rows the selection matches right now — the preview the user
    /// confirms against, and the source of the count sent as `confirm_count`.
    rows: Vec<PruneCandidateItem>,
    total: i64,
}

async fn resolve_selection(
    ctx: &Ctx,
    args: &PruneSelectorArgs,
    states: &str,
) -> CliResult<Selection> {
    if !args.filter && args.ids.is_empty() {
        return Err(CliError::Usage(
            "give candidate ids (@N from `ovis prune ls`) or --filter with filter flags".into(),
        ));
    }

    if args.filter {
        let connector_id = match &args.connector {
            Some(reference) => Some(resolve::connector(ctx, reference).await?.connector_id()),
            None => None,
        };
        let filter = PruneCandidateFilterBody {
            state: Some(states.to_string()),
            detector: args.detector.clone(),
            connector_id,
            min_confidence: args.min_confidence,
            recrawl_risk: args.risky.then_some(true),
            scan_id: args.scan,
        };

        // Preview through the same GET filters the server applies to the
        // mutation, so the count we confirm is the count that acts.
        let mut query = QueryBuilder::new();
        query.push("state", states);
        query.push_opt("detector", filter.detector.as_deref());
        query.push_opt("connector_id", filter.connector_id);
        query.push_opt("min_confidence", filter.min_confidence);
        if args.risky {
            query.push("recrawl_risk", true);
        }
        query.push_opt("scan_id", filter.scan_id);
        query.push("limit", 500);
        let preview = ctx.api.prune_candidates(&query.build()).await?;

        Ok(Selection {
            ids: None,
            filter: Some(filter),
            total: preview.total,
            rows: preview.items,
        })
    } else {
        let ids = resolve_candidate_ids(&args.ids)?;
        let mut rows = Vec::new();
        for id in &ids {
            rows.push(ctx.api.prune_candidate(*id).await?.item);
        }
        Ok(Selection {
            total: ids.len() as i64,
            ids: Some(ids),
            filter: None,
            rows,
        })
    }
}

/// The stage/delete confirmation: count, chunk sum, risk breakdown — then a
/// y/N prompt, with the typed count for big batches that `-y` never waives.
async fn confirm_bulk(
    ctx: &Ctx,
    verb: &str,
    selection: &Selection,
    confirm_count: Option<i64>,
    always_typed: bool,
) -> CliResult<i64> {
    let total = selection.total;
    let status = ctx.api.prune_status().await?;
    let big_batch = status.limits.big_batch;

    let sampled = selection.rows.len() as i64;
    let chunk_sum: i64 = selection
        .rows
        .iter()
        .filter_map(|r| r.chunk_count)
        .map(i64::from)
        .sum();
    let risky = selection.rows.iter().filter(|r| r.recrawl_risk).count() as i64;

    let mut summary = format!("{verb} {} document{}", thousands(total), plural(total));
    if sampled == total {
        summary.push_str(&format!(" ({} chunks", thousands(chunk_sum)));
    } else {
        summary.push_str(&format!(
            " (≥{} chunks across the first {} sampled",
            thousands(chunk_sum),
            thousands(sampled)
        ));
    }
    if risky > 0 {
        summary.push_str(&format!(
            ", {} at recrawl risk — an active connector will likely bring them back",
            thousands(risky)
        ));
    }
    summary.push(')');
    ctx.out.note(&summary);

    if let Some(confirmed) = confirm_count {
        if confirmed != total {
            return Err(CliError::Usage(format!(
                "--confirm-count {confirmed} does not match the current selection of {total}; \
                 nothing was changed. Re-check and pass --confirm-count {total}"
            )));
        }
        return Ok(total);
    }

    let needs_typed_count = total > big_batch || always_typed;
    if needs_typed_count {
        // -y deliberately does not skip this: the operation is thousands of
        // documents, or a scheduled deletion. Same rule as connector delete.
        if ctx.interaction.no_input || !prompt::can_prompt() {
            return Err(CliError::NeedsConfirmation(format!(
                "{verb} of {total} documents needs the typed count; pass --confirm-count {total} \
                 to supply it non-interactively"
            )));
        }
        let answer = prompt::ask_line(
            &format!("Type the document count to confirm ({total})"),
            None,
        )?;
        if answer.trim() != total.to_string() {
            return Err(CliError::NeedsConfirmation(format!(
                "'{answer}' does not match {total}; nothing was changed"
            )));
        }
        return Ok(total);
    }

    if !prompt::confirm(&format!("{summary}?"), ctx.interaction)? {
        return Err(CliError::NeedsConfirmation("cancelled".into()));
    }
    Ok(total)
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn report_bulk(ctx: &Ctx, status: u16, response: &PruneBulkResponse, done: &str) -> CliResult<()> {
    match ctx.out.format {
        Format::Json => ctx.out.json(response)?,
        Format::Yaml => ctx.out.yaml(response)?,
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(response))?,
        Format::Table | Format::Csv => {
            let mut line = format!("{done}: {} of {}", response.changed, response.requested);
            if let Some(via) = &response.boost_hidden_via {
                line.push_str(&format!(" · hidden via {via}"));
            }
            if let Some(expires) = &response.stage_expires_at {
                line.push_str(&format!(
                    " · grace ends {}",
                    timestamp(expires, ctx.relative_time())
                ));
            }
            ctx.out.note(line);
            for failure in &response.failed {
                ctx.out.warn(format!(
                    "  failed {} (candidate {}): {}",
                    failure.document_id, failure.candidate_id, failure.code
                ));
            }
        }
    }

    if status == 207 || !response.success {
        return Err(CliError::PartialFailure(format!(
            "{} of {} changed; {} failed",
            response.changed,
            response.requested,
            response.failed.len()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// stage / dismiss / restore / delete
// ---------------------------------------------------------------------------

pub async fn stage(ctx: &Ctx, args: &PruneSelectorArgs) -> CliResult<()> {
    let selection = resolve_selection(ctx, args, "candidate").await?;
    if selection.total == 0 {
        return usage("the selection matches no open candidates");
    }
    let confirmed = confirm_bulk(ctx, "stage", &selection, args.confirm_count, false).await?;

    let (status, response) = ctx
        .api
        .prune_stage(&PruneStageRequest {
            ids: selection.ids.clone(),
            filter: selection.filter.clone(),
            confirm_count: confirmed,
        })
        .await?;
    report_bulk(ctx, status, &response, "staged (hidden from search, data intact)")?;
    if matches!(ctx.out.format, Format::Table) {
        ctx.out
            .footer("watch: ovis prune staged · undo: ovis prune restore @N");
    }
    Ok(())
}

pub async fn dismiss(ctx: &Ctx, references: &[String], forever: bool) -> CliResult<()> {
    let ids = resolve_candidate_ids(references)?;
    let (status, response) = ctx
        .api
        .prune_dismiss(&PruneDismissRequest {
            ids: Some(ids),
            filter: None,
            exclude_future: forever,
            confirm_count: None,
        })
        .await?;
    let done = if forever {
        "dismissed (excluded from all future scans)"
    } else {
        "dismissed"
    };
    report_bulk(ctx, status, &response, done)
}

pub async fn restore(ctx: &Ctx, references: &[String], all_staged: bool) -> CliResult<()> {
    let request = if all_staged {
        PruneRestoreRequest {
            ids: None,
            filter: Some(PruneCandidateFilterBody {
                state: Some("staged".into()),
                ..Default::default()
            }),
            confirm_count: None,
        }
    } else {
        if references.is_empty() {
            return usage("give candidate ids (@N) or --all-staged");
        }
        PruneRestoreRequest {
            ids: Some(resolve_candidate_ids(references)?),
            filter: None,
            confirm_count: None,
        }
    };

    // Restore is the safe direction: no confirmation.
    let (status, response) = ctx.api.prune_restore(&request).await?;
    report_bulk(ctx, status, &response, "restored (exactly as before staging)")
}

pub async fn delete(ctx: &Ctx, args: &PruneDeleteArgs) -> CliResult<()> {
    // Deletion acts on open candidates and staged rows alike: candidates are
    // staged first (full grace), staged rows come due now.
    let selection = resolve_selection(ctx, &args.selector, "candidate,staged").await?;
    if selection.total == 0 {
        return usage("the selection matches no candidates or staged documents");
    }

    // The typed count is unconditional for scheduled deletion with --filter,
    // and for anything beyond big_batch. `-y` never waives it.
    let always_typed = args.selector.filter;
    let confirmed = confirm_bulk(
        ctx,
        "schedule deletion of",
        &selection,
        args.selector.confirm_count,
        always_typed,
    )
    .await?;

    let (status, response) = ctx
        .api
        .prune_schedule_delete(&PruneScheduleDeleteRequest {
            ids: selection.ids.clone(),
            filter: selection.filter.clone(),
            confirm_count: confirmed,
            remember: args.remember.then_some(true),
        })
        .await?;

    report_bulk(ctx, status, &response, "scheduled for deletion")?;
    if matches!(ctx.out.format, Format::Table) {
        match &response.stage_expires_at {
            Some(expires) => ctx.out.footer(format!(
                "the reaper deletes after the grace period (ends {}) · restore until then: \
                 ovis prune restore @N · watch: ovis prune staged",
                timestamp(expires, ctx.relative_time())
            )),
            None => ctx.out.footer(
                "the reaper executes at its next cycle · restore until then: ovis prune \
                 restore @N",
            ),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// status / log / exclusions
// ---------------------------------------------------------------------------

pub async fn status(ctx: &Ctx) -> CliResult<()> {
    let status = ctx.api.prune_status().await?;

    match ctx.out.format {
        Format::Json => ctx.out.json(&status)?,
        Format::Yaml => ctx.out.yaml(&status)?,
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(&status))?,
        Format::Table | Format::Csv => render_status(ctx, &status)?,
    }

    if status.reaper.halted {
        return Err(CliError::Degraded(format!(
            "the reaper is halted: {}",
            status
                .reaper
                .halted_reason
                .as_deref()
                .unwrap_or("unknown reason")
        )));
    }
    Ok(())
}

fn render_status(ctx: &Ctx, status: &PruneStatusResponse) -> CliResult<()> {
    let relative = ctx.relative_time();
    let mut grid = Grid::new(vec!["field".into(), "value".into()]);
    let mut push = |k: &str, cell: GridCell| grid.push(vec![GridCell::plain(k), cell]);

    push("candidates open", GridCell::plain(thousands(status.candidates)));
    push(
        "staged",
        match status.soonest_expiry {
            Some(expiry) => GridCell::plain(format!(
                "{} (soonest grace ends {})",
                thousands(status.staged),
                timestamp(&expiry, relative)
            )),
            None => GridCell::plain(thousands(status.staged)),
        },
    );
    push("deleting now", GridCell::plain(thousands(status.deleting)));
    push("deleted (7 days)", GridCell::plain(thousands(status.deleted_7d)));
    push("deleted (total)", GridCell::plain(thousands(status.deleted_total)));
    push("dismissed", GridCell::plain(thousands(status.dismissed_total)));
    push("restored", GridCell::plain(thousands(status.restored_total)));
    push("exclusion list", GridCell::plain(thousands(status.exclusions)));

    let reaper = &status.reaper;
    let reaper_cell = if reaper.halted {
        GridCell::toned(
            format!(
                "HALTED: {}",
                reaper.halted_reason.as_deref().unwrap_or("unknown")
            ),
            Tone::Error,
        )
    } else if reaper.deferred > 0 {
        GridCell::toned(
            format!(
                "deferred {} ({})",
                reaper.deferred,
                reaper.deferred_reason.as_deref().unwrap_or("")
            ),
            Tone::Warn,
        )
    } else {
        match reaper.next_run_at {
            Some(next) => GridCell::plain(format!("next run {}", timestamp(&next, relative))),
            None => GridCell::toned("not yet run", Tone::Dim),
        }
    };
    push("reaper", reaper_cell);
    push(
        "deletion rate",
        GridCell::plain(format!(
            "{} in the last hour (limit {}/hour, batches of {})",
            thousands(reaper.deleted_last_hour),
            thousands(status.limits.max_docs_per_hour),
            status.limits.reaper_batch_size,
        )),
    );
    push(
        "grace period",
        GridCell::plain(format!("{} days", status.limits.grace_days)),
    );
    if let Some(scan) = &status.active_scan {
        push(
            "scan",
            GridCell::plain(format!(
                "#{} {} — {}{}",
                scan.id,
                scan.status,
                thousands(scan.examined),
                scan.total
                    .map(|t| format!(" of {}", thousands(t)))
                    .unwrap_or_default()
            )),
        );
    }
    ctx.out.grid(&grid)?;

    if status.staged_expiring_24h > 0 {
        ctx.out.warn(format!(
            "{} staged document{} reach the end of grace within 24 hours",
            thousands(status.staged_expiring_24h),
            plural(status.staged_expiring_24h)
        ));
    }
    Ok(())
}

pub async fn log(
    ctx: &Ctx,
    since: Option<&str>,
    action: Option<&str>,
    limit: Option<i64>,
    page: Option<i64>,
) -> CliResult<()> {
    let mut query = QueryBuilder::new();
    if let Some(since) = since {
        let ts = resolve::parse_when(since).map_err(CliError::Usage)?;
        query.push("since", ts.to_rfc3339());
    }
    query.push_opt("action", action);
    query.push_opt("limit", limit.or(Some(ctx.out.default_limit())));
    query.push_opt("page", page);

    let response = ctx.api.prune_audit(&query.build()).await?;
    match ctx.out.format {
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let relative = ctx.relative_time();
            let mut grid = Grid::new(
                ["when", "actor", "action", "document", "detail"]
                    .map(String::from)
                    .to_vec(),
            );
            for entry in &response.items {
                let detail = entry
                    .detail
                    .as_ref()
                    .map(summarise_detail)
                    .unwrap_or_default();
                grid.push(vec![
                    GridCell::plain(timestamp(&entry.at, relative)),
                    GridCell::plain(&entry.actor),
                    GridCell::toned(&entry.action, action_tone(&entry.action)),
                    GridCell::plain(entry.document_id.as_deref().unwrap_or("—")),
                    GridCell::plain(detail),
                ]);
            }
            ctx.out.grid(&grid)?;
            ctx.out
                .footer(format!("{} audit entries", thousands(response.total)));
        }
    }
    Ok(())
}

fn action_tone(action: &str) -> Tone {
    match action {
        "deleted" | "halted" | "delete_failed" | "scan_failed" => Tone::Error,
        "staged" | "scheduled" | "deferred" | "restaged_recrawled" => Tone::Warn,
        "restored" | "reaper_resumed" | "scan_finished" => Tone::Ok,
        _ => Tone::Dim,
    }
}

fn summarise_detail(detail: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    for key in [
        "count",
        "reason",
        "chunks_deleted",
        "index_cleanup_pending",
        "remember",
        "expedited",
        "via",
        "error",
    ] {
        if let Some(value) = detail.get(key) {
            if value.as_bool() == Some(false) || value.is_null() {
                continue;
            }
            parts.push(format!("{key}={value}"));
        }
    }
    parts.join(" ")
}

pub async fn exclusions(ctx: &Ctx, limit: Option<i64>, page: Option<i64>) -> CliResult<()> {
    let mut query = QueryBuilder::new();
    query.push_opt("limit", limit.or(Some(ctx.out.default_limit())));
    query.push_opt("page", page);
    let response = ctx.api.prune_exclusions(&query.build()).await?;

    match ctx.out.format {
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let relative = ctx.relative_time();
            let mut grid = Grid::new(
                ["document", "reason", "note", "added"].map(String::from).to_vec(),
            );
            for item in &response.items {
                grid.push(vec![
                    GridCell::plain(&item.document_id),
                    GridCell::plain(&item.reason),
                    GridCell::plain(item.note.as_deref().unwrap_or("—")),
                    GridCell::plain(timestamp(&item.created_at, relative)),
                ]);
            }
            ctx.out.grid(&grid)?;
            ctx.out.footer(format!(
                "{} excluded · remembered deletions are re-staged automatically if recrawled",
                thousands(response.total)
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// rules
// ---------------------------------------------------------------------------

pub async fn rules(ctx: &Ctx, action: &PruneRulesCommand) -> CliResult<()> {
    match action {
        PruneRulesCommand::List => rules_list(ctx).await,
        PruneRulesCommand::Add {
            name,
            kind,
            pattern,
            confidence,
        } => {
            let rule = ctx
                .api
                .prune_rule_create(&PruneRuleCreate {
                    name: name.clone(),
                    kind: kind.clone(),
                    body: serde_json::json!({ "pattern": pattern, "confidence": confidence }),
                    enabled: false,
                })
                .await?;
            ctx.out.note(format!(
                "rule '{}' created (disabled) · preview it: ovis prune rules preview {} · \
                 then: ovis prune rules enable {}",
                rule.name, rule.id, rule.id
            ));
            Ok(())
        }
        PruneRulesCommand::Preview { rule } => {
            let rule = find_rule(ctx, rule).await?;
            let preview = ctx.api.prune_rule_preview(rule.id).await?;
            match ctx.out.format {
                Format::Json => ctx.out.json(&preview)?,
                Format::Yaml => ctx.out.yaml(&preview)?,
                Format::Ndjson => ctx.out.ndjson(&preview.sample)?,
                Format::Table | Format::Csv => {
                    let mut grid = Grid::new(vec!["matched on".into(), "document".into()]);
                    for hit in &preview.sample {
                        grid.push(vec![
                            GridCell::plain(&hit.matched_on),
                            GridCell::plain(hit.semantic_id.as_deref().unwrap_or(&hit.document_id)),
                        ]);
                    }
                    ctx.out.grid(&grid)?;
                    let scope_note = if preview.complete {
                        format!("{} matched of {} scanned", preview.matched, thousands(preview.scanned))
                    } else {
                        format!(
                            "≥{} matched in the first {} scanned (preview cap; a scan covers \
                             everything)",
                            preview.matched,
                            thousands(preview.scanned)
                        )
                    };
                    ctx.out.footer(scope_note);
                }
            }
            Ok(())
        }
        PruneRulesCommand::Enable { rule } => set_rule_enabled(ctx, rule, true).await,
        PruneRulesCommand::Disable { rule } => set_rule_enabled(ctx, rule, false).await,
        PruneRulesCommand::Delete { rule } => {
            let rule = find_rule(ctx, rule).await?;
            ctx.api.prune_rule_delete(rule.id).await?;
            ctx.out.note(format!("rule '{}' deleted", rule.name));
            Ok(())
        }
    }
}

async fn rules_list(ctx: &Ctx) -> CliResult<()> {
    let rules = ctx.api.prune_rules().await?;
    match ctx.out.format {
        Format::Json => ctx.out.json(&rules)?,
        Format::Yaml => ctx.out.yaml(&rules)?,
        Format::Ndjson => ctx.out.ndjson(&rules)?,
        Format::Table | Format::Csv => {
            let mut grid = Grid::new(
                ["id", "name", "kind", "enabled", "pattern / body"]
                    .map(String::from)
                    .to_vec(),
            );
            for rule in &rules {
                let body = rule
                    .body
                    .get("pattern")
                    .and_then(|p| p.as_str())
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "detector configuration".to_string());
                grid.push(vec![
                    GridCell::plain(rule.id.to_string()),
                    GridCell::plain(&rule.name),
                    GridCell::plain(&rule.kind),
                    if rule.enabled {
                        GridCell::toned("yes", Tone::Ok)
                    } else {
                        GridCell::toned("no", Tone::Dim)
                    },
                    GridCell::plain(body),
                ]);
            }
            ctx.out.grid(&grid)?;
            ctx.out.footer(
                "rules start disabled · preview: ovis prune rules preview ID · enable: ovis \
                 prune rules enable ID",
            );
        }
    }
    Ok(())
}

async fn find_rule(ctx: &Ctx, reference: &str) -> CliResult<PruneRuleItem> {
    let rules = ctx.api.prune_rules().await?;
    if let Ok(id) = reference.parse::<i64>() {
        if let Some(rule) = rules.iter().find(|r| r.id == id) {
            return Ok(rule.clone());
        }
    }
    let matches: Vec<&PruneRuleItem> = rules.iter().filter(|r| r.name == reference).collect();
    match matches.as_slice() {
        [rule] => Ok((*rule).clone()),
        [] => Err(CliError::Usage(format!(
            "no rule named or numbered '{reference}'; see `ovis prune rules list`"
        ))),
        _ => Err(CliError::Usage(format!(
            "'{reference}' is ambiguous; use the rule id"
        ))),
    }
}

async fn set_rule_enabled(ctx: &Ctx, reference: &str, enabled: bool) -> CliResult<()> {
    let rule = find_rule(ctx, reference).await?;
    let updated = ctx
        .api
        .prune_rule_patch(
            rule.id,
            &PruneRulePatch {
                name: None,
                body: None,
                enabled: Some(enabled),
            },
        )
        .await?;
    ctx.out.note(format!(
        "rule '{}' is now {}",
        updated.name,
        if updated.enabled { "enabled" } else { "disabled" }
    ));
    Ok(())
}

// ---------------------------------------------------------------------------
// config export / import
// ---------------------------------------------------------------------------

pub async fn config(ctx: &Ctx, action: &PruneConfigCommand) -> CliResult<()> {
    match action {
        PruneConfigCommand::Export { file } => {
            let yaml = ctx.api.prune_config_export().await?;
            if file == "-" {
                ctx.out.print(&yaml)?;
            } else {
                std::fs::write(file, &yaml).map_err(|e| {
                    CliError::Other(anyhow::anyhow!("cannot write {file}: {e}"))
                })?;
                ctx.out.note(format!("wrote the detector config to {file}"));
            }
            Ok(())
        }
        PruneConfigCommand::Import { file } => {
            let yaml = if file == "-" {
                use std::io::Read;
                let mut buffer = String::new();
                std::io::stdin().read_to_string(&mut buffer)?;
                buffer
            } else {
                std::fs::read_to_string(file).map_err(|e| {
                    CliError::Other(anyhow::anyhow!("cannot read {file}: {e}"))
                })?
            };
            let rule = ctx.api.prune_config_import(&yaml).await?;
            ctx.out.note(format!(
                "detector config imported (stored as rule '{}', enabled)",
                rule.name
            ));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_sorts_parse_to_server_values() {
        assert_eq!(parse_prune_sort("confidence").unwrap(), "confidence_desc");
        assert_eq!(parse_prune_sort("chunks:asc").unwrap(), "chunks_asc");
        assert_eq!(parse_prune_sort("expiry").unwrap(), "expiry_asc");
        assert!(parse_prune_sort("chunk_desc").is_err());
    }

    #[test]
    fn reason_chips_are_compact_and_specific() {
        let reasons = vec![
            PruneReason {
                detector: "duplicate".into(),
                code: "near_duplicate_of".into(),
                detail: String::new(),
                confidence: 0.94,
                evidence: serde_json::json!({}),
            },
            PruneReason {
                detector: "language".into(),
                code: "lang_not_allowed".into(),
                detail: String::new(),
                confidence: 0.98,
                evidence: serde_json::json!({ "detected": "deu" }),
            },
            PruneReason {
                detector: "url_rule".into(),
                code: "calendar-pages".into(),
                detail: String::new(),
                confidence: 0.8,
                evidence: serde_json::json!({}),
            },
            PruneReason {
                detector: "thin".into(),
                code: "chunkless_stub".into(),
                detail: String::new(),
                confidence: 0.9,
                evidence: serde_json::json!({}),
            },
        ];
        let chips = reason_chips(&reasons);
        assert!(chips.contains("dup 94%"), "{chips}");
        assert!(chips.contains("lang deu 0.98"), "{chips}");
        assert!(chips.contains("rule calendar-pages"), "{chips}");
        assert!(chips.contains("stub"), "{chips}");
    }

    #[test]
    fn candidate_references_must_be_numeric_ids_or_handles() {
        assert!(resolve_candidate_id("https://example.com/a").is_err());
        assert_eq!(resolve_candidate_id("42").unwrap(), 42);
    }
}
