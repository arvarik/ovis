//! Turning wire structs into grids.
//!
//! One place per entity, shared by every command that shows it, so `page list`,
//! `connector docs` and the TUI cannot drift into showing the same document
//! three different ways.

use ovis_core::api_types::*;

use crate::ctx::Ctx;
use crate::error::{CliError, CliResult};
use crate::output::style::{status_tone, Tone};
use crate::output::table::{select_columns, ColSpec, Grid, GridCell};
use crate::output::{bytes, render_snippet, thousands, timestamp, timestamp_opt};

/// Check `--columns` before the request goes out.
///
/// Column selection otherwise only fails at render time, so a typo against an
/// unreachable server reported "cannot reach the server" (exit 12) and hid the
/// actual mistake. A usage error should not need a working server to be found.
pub fn validate_columns(ctx: &Ctx, specs: &[ColSpec]) -> CliResult<()> {
    if ctx.out.columns.is_some() {
        select_columns(specs, ctx.out.wide, ctx.out.columns.as_deref())
            .map(|_| ())
            .map_err(CliError::Usage)?;
    }
    Ok(())
}

/// `chunk_count: null` means Onyx has not counted this document yet, which is
/// *not* zero. Rendering it as 0 would be a small lie with real consequences —
/// `--stubs` deliberately excludes these rows.
fn chunk_cell(count: Option<i32>) -> GridCell {
    match count {
        Some(0) => GridCell::toned("0", Tone::Dim),
        Some(n) => GridCell::plain(n.to_string()),
        None => GridCell::toned("—", Tone::Dim),
    }
}

fn bool_cell(value: bool, true_tone: Tone) -> GridCell {
    if value {
        GridCell::toned("yes", true_tone)
    } else {
        GridCell::toned("no", Tone::Dim)
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

pub const PAGE_COLUMNS: &[ColSpec] = &[
    ColSpec::new("#"),
    ColSpec::new("title"),
    ColSpec::new("connector"),
    ColSpec::new("chunks"),
    ColSpec::new("updated"),
    ColSpec::new("url"),
    ColSpec::wide("id"),
    ColSpec::wide("source"),
    ColSpec::wide("boost"),
    ColSpec::wide("hidden"),
    ColSpec::wide("last_modified"),
    ColSpec::wide("doc_updated"),
];

/// `first_handle` numbers the `#` column; pass `None` to omit handles entirely
/// (`--all`, where 1.6 M numbered rows would be meaningless).
pub fn pages(ctx: &Ctx, items: &[PageListItem], first_handle: Option<usize>) -> CliResult<Grid> {
    let mut chosen = select_columns(PAGE_COLUMNS, ctx.out.wide, ctx.out.columns.as_deref())
        .map_err(CliError::Usage)?;
    if first_handle.is_none() && ctx.out.columns.is_none() {
        chosen.retain(|c| *c != "#");
    }

    let relative = ctx.relative_time();
    let mut grid = Grid::new(chosen.iter().map(|c| header(c)).collect());

    for (offset, item) in items.iter().enumerate() {
        let handle = first_handle.map(|first| first + offset);
        let row = chosen
            .iter()
            .map(|col| match *col {
                "#" => GridCell::toned(
                    handle.map(|n| format!("@{n}")).unwrap_or_default(),
                    Tone::Dim,
                ),
                "title" => GridCell::plain(&item.semantic_id),
                "connector" => match &item.connector_name {
                    Some(name) => GridCell::plain(name),
                    None => GridCell::toned("—", Tone::Dim),
                },
                "chunks" => chunk_cell(item.chunk_count),
                "updated" => GridCell::plain(timestamp(&item.updated_at, relative)),
                "url" => GridCell::plain(item.link.as_deref().unwrap_or(&item.id)),
                "id" => GridCell::plain(&item.id),
                "source" => GridCell::plain(item.connector_source.as_deref().unwrap_or("—")),
                "boost" => GridCell::plain(item.boost.to_string()),
                "hidden" => bool_cell(item.hidden, Tone::Warn),
                "last_modified" => GridCell::plain(timestamp(&item.last_modified, relative)),
                "doc_updated" => {
                    GridCell::plain(timestamp_opt(item.doc_updated_at.as_ref(), relative))
                }
                other => GridCell::plain(format!("<{other}?>")),
            })
            .collect();
        grid.push(row);
    }
    Ok(grid)
}

/// The metadata card for `page view`.
pub fn page_detail(ctx: &Ctx, detail: &PageDetail) -> Grid {
    let relative = ctx.relative_time();
    let item = &detail.item;
    let mut grid = Grid::new(vec!["FIELD".into(), "VALUE".into()]);

    let mut row = |k: &str, v: GridCell| grid.push(vec![GridCell::plain(k), v]);

    row("title", GridCell::plain(&item.semantic_id));
    row("id", GridCell::plain(&item.id));
    if let Some(link) = &item.link {
        if link != &item.id {
            row("link", GridCell::plain(link));
        }
    }
    row(
        "connector",
        match (&item.connector_name, &detail.cc_pair_status) {
            (Some(name), Some(status)) => {
                GridCell::toned(format!("{name} ({status})"), status_tone(status))
            }
            (Some(name), None) => GridCell::plain(name),
            _ => GridCell::toned("—", Tone::Dim),
        },
    );
    if let Some(source) = &item.connector_source {
        row("source", GridCell::plain(source));
    }
    row("chunks", chunk_cell(item.chunk_count));
    row("boost", GridCell::plain(item.boost.to_string()));
    row("hidden", bool_cell(item.hidden, Tone::Warn));
    row(
        "updated",
        GridCell::plain(timestamp(&item.updated_at, relative)),
    );
    row(
        "last_modified",
        GridCell::plain(timestamp(&item.last_modified, relative)),
    );
    if item.doc_updated_at.is_some() {
        row(
            "doc_updated_at",
            GridCell::plain(timestamp_opt(item.doc_updated_at.as_ref(), relative)),
        );
    }
    if let Some(synced) = &detail.last_synced {
        row("last_synced", GridCell::plain(timestamp(synced, relative)));
    }
    if let Some(owners) = &detail.primary_owners {
        if !owners.is_empty() {
            row("primary_owners", GridCell::plain(owners.join(", ")));
        }
    }
    if let Some(owners) = &detail.secondary_owners {
        if !owners.is_empty() {
            row("secondary_owners", GridCell::plain(owners.join(", ")));
        }
    }
    if !detail.tags.is_empty() {
        let rendered: Vec<String> = detail
            .tags
            .iter()
            .map(|t| format!("{}={}", t.key, t.value))
            .collect();
        row("tags", GridCell::plain(rendered.join(", ")));
    }
    if let Some(hash) = &detail.content_hash {
        row("content_hash", GridCell::plain(hash));
    }
    if detail.from_ingestion_api == Some(true) {
        row("from_ingestion_api", GridCell::toned("yes", Tone::Info));
    }

    // Both of these are the response telling the truth about something
    // uncomfortable, so neither is hidden behind --wide.
    if !detail.pg_row {
        row(
            "pg_row",
            GridCell::toned("missing — the index holds orphaned chunks", Tone::Error),
        );
    }
    if detail.recrawl_risk {
        row(
            "recrawl_risk",
            GridCell::toned(
                "yes — the connector is active, so a delete is likely to be undone",
                Tone::Warn,
            ),
        );
    }

    grid
}

// ---------------------------------------------------------------------------
// Chunks
// ---------------------------------------------------------------------------

pub const CHUNK_COLUMNS: &[ColSpec] = &[
    ColSpec::new("#"),
    ColSpec::new("words"),
    ColSpec::new("title"),
    ColSpec::new("text"),
    ColSpec::wide("updated"),
    ColSpec::wide("hidden"),
];

pub fn chunks(ctx: &Ctx, items: &[ChunkItem], full: bool) -> CliResult<Grid> {
    let chosen = select_columns(CHUNK_COLUMNS, ctx.out.wide, ctx.out.columns.as_deref())
        .map_err(CliError::Usage)?;
    let relative = ctx.relative_time();
    let mut grid = Grid::new(chosen.iter().map(|c| header(c)).collect());

    for item in items {
        let text = if full {
            item.content.clone().unwrap_or_default()
        } else {
            item.blurb
                .clone()
                .or_else(|| item.content.clone())
                .unwrap_or_default()
        };
        let row = chosen
            .iter()
            .map(|col| match *col {
                "#" => GridCell::plain(item.chunk_index.to_string()),
                "words" => GridCell::plain(
                    item.token_estimate
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "—".into()),
                ),
                "title" => GridCell::plain(item.title.as_deref().unwrap_or("—")),
                "text" => GridCell::plain(text.replace('\n', " ")),
                "updated" => GridCell::plain(timestamp_opt(item.last_updated.as_ref(), relative)),
                "hidden" => bool_cell(item.hidden.unwrap_or(false), Tone::Warn),
                other => GridCell::plain(format!("<{other}?>")),
            })
            .collect();
        grid.push(row);
    }
    Ok(grid)
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

pub const SEARCH_COLUMNS: &[ColSpec] = &[
    ColSpec::new("#"),
    ColSpec::new("score"),
    ColSpec::new("title"),
    ColSpec::new("connector"),
    ColSpec::new("snippet"),
    ColSpec::wide("id"),
    ColSpec::wide("chunk"),
    ColSpec::wide("chunks"),
    ColSpec::wide("updated"),
];

pub fn search_hits(ctx: &Ctx, items: &[SearchHit], first_handle: usize) -> CliResult<Grid> {
    let chosen = select_columns(SEARCH_COLUMNS, ctx.out.wide, ctx.out.columns.as_deref())
        .map_err(CliError::Usage)?;
    let relative = ctx.relative_time();
    let mut grid = Grid::new(chosen.iter().map(|c| header(c)).collect());

    for (offset, hit) in items.iter().enumerate() {
        let row = chosen
            .iter()
            .map(|col| match *col {
                "#" => GridCell::toned(format!("@{}", first_handle + offset), Tone::Dim),
                "score" => GridCell::plain(format!("{:.2}", hit.score)),
                "title" => GridCell::plain(
                    hit.semantic_id
                        .as_deref()
                        .unwrap_or(hit.document_id.as_str()),
                ),
                "connector" => GridCell::plain(hit.connector_name.as_deref().unwrap_or("—")),
                "snippet" => GridCell::plain(render_snippet(
                    hit.snippet.as_deref().unwrap_or(""),
                    ctx.out.color,
                )),
                "id" => GridCell::plain(&hit.document_id),
                "chunk" => GridCell::plain(
                    hit.chunk_index
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "—".into()),
                ),
                "chunks" => chunk_cell(hit.chunk_count),
                "updated" => GridCell::plain(timestamp_opt(hit.updated_at.as_ref(), relative)),
                other => GridCell::plain(format!("<{other}?>")),
            })
            .collect();
        grid.push(row);
    }
    Ok(grid)
}

// ---------------------------------------------------------------------------
// Connectors
// ---------------------------------------------------------------------------

pub const CONNECTOR_COLUMNS: &[ColSpec] = &[
    ColSpec::new("#"),
    ColSpec::new("name"),
    ColSpec::new("source"),
    ColSpec::new("status"),
    ColSpec::new("docs"),
    ColSpec::new("last_success"),
    ColSpec::new("last_attempt"),
    ColSpec::wide("cc_pair"),
    ColSpec::wide("connector_id"),
    ColSpec::wide("refresh"),
    ColSpec::wide("trigger"),
    ColSpec::wide("error"),
];

/// The status cell folds in the two flags that change what a status *means*:
/// parked (the resilience cron deliberately skips it) and repeated errors.
fn connector_status_cell(c: &ConnectorSummary) -> GridCell {
    let mut text = c.status.clone();
    if c.parked {
        text.push_str(" ⏸parked");
    }
    if c.in_repeated_error_state {
        text.push_str(" ⚠errors");
    }
    let tone = if c.in_repeated_error_state {
        Tone::Error
    } else if c.parked {
        Tone::Warn
    } else {
        status_tone(&c.status)
    };
    GridCell::toned(text, tone)
}

pub fn connectors(
    ctx: &Ctx,
    items: &[ConnectorSummary],
    first_handle: Option<usize>,
) -> CliResult<Grid> {
    let mut chosen = select_columns(CONNECTOR_COLUMNS, ctx.out.wide, ctx.out.columns.as_deref())
        .map_err(CliError::Usage)?;
    if first_handle.is_none() && ctx.out.columns.is_none() {
        chosen.retain(|c| *c != "#");
    }
    let relative = ctx.relative_time();
    let mut grid = Grid::new(chosen.iter().map(|c| header(c)).collect());

    for (offset, c) in items.iter().enumerate() {
        let handle = first_handle.map(|first| first + offset);
        let row = chosen
            .iter()
            .map(|col| match *col {
                "#" => GridCell::toned(
                    handle.map(|n| format!("@{n}")).unwrap_or_default(),
                    Tone::Dim,
                ),
                "name" => GridCell::plain(&c.name),
                "source" => GridCell::plain(&c.source),
                "status" => connector_status_cell(c),
                "docs" => GridCell::plain(thousands(c.doc_count)),
                "last_success" => GridCell::plain(timestamp_opt(
                    c.last_successful_index_time.as_ref(),
                    relative,
                )),
                "last_attempt" => match &c.last_attempt {
                    Some(a) => {
                        let status = a.status.as_deref().unwrap_or("—");
                        GridCell::toned(
                            format!(
                                "{status} {}",
                                timestamp_opt(a.time_updated.as_ref(), relative)
                            ),
                            status_tone(status),
                        )
                    }
                    None => GridCell::toned("—", Tone::Dim),
                },
                "cc_pair" => GridCell::plain(c.cc_pair_id.to_string()),
                "connector_id" => GridCell::plain(c.connector_id.to_string()),
                "refresh" => GridCell::plain(
                    c.refresh_freq_secs
                        .map(|s| {
                            humantime::format_duration(std::time::Duration::from_secs(
                                s.max(0) as u64
                            ))
                            .to_string()
                        })
                        .unwrap_or_else(|| "—".into()),
                ),
                "trigger" => GridCell::plain(c.indexing_trigger.as_deref().unwrap_or("—")),
                "error" => match c.last_attempt.as_ref().and_then(|a| a.error_msg.as_deref()) {
                    Some(msg) => GridCell::toned(msg.trim(), Tone::Dim),
                    None => GridCell::plain(""),
                },
                other => GridCell::plain(format!("<{other}?>")),
            })
            .collect();
        grid.push(row);
    }
    Ok(grid)
}

pub fn connector_detail(ctx: &Ctx, detail: &ConnectorDetail) -> Grid {
    let relative = ctx.relative_time();
    let s = &detail.summary;
    let mut grid = Grid::new(vec!["FIELD".into(), "VALUE".into()]);
    let mut row = |k: &str, v: GridCell| grid.push(vec![GridCell::plain(k), v]);

    row("name", GridCell::plain(&s.name));
    row("cc_pair_id", GridCell::plain(s.cc_pair_id.to_string()));
    row("connector_id", GridCell::plain(s.connector_id.to_string()));
    row("source", GridCell::plain(&s.source));
    row("status", connector_status_cell(s));
    if s.parked {
        row(
            "parked",
            GridCell::toned(
                "the resilience cron marked this cc-pair done; run-once needs \
                 --acknowledge-parked",
                Tone::Warn,
            ),
        );
    }
    row("documents", GridCell::plain(thousands(s.doc_count)));
    row(
        "last_success",
        GridCell::plain(timestamp_opt(
            s.last_successful_index_time.as_ref(),
            relative,
        )),
    );
    if let Some(refresh) = s.refresh_freq_secs {
        row(
            "refresh_freq",
            GridCell::plain(
                humantime::format_duration(std::time::Duration::from_secs(refresh.max(0) as u64))
                    .to_string(),
            ),
        );
    }
    if let Some(prune) = detail.prune_freq_secs {
        row(
            "prune_freq",
            GridCell::plain(
                humantime::format_duration(std::time::Duration::from_secs(prune.max(0) as u64))
                    .to_string(),
            ),
        );
    }
    if let Some(trigger) = &s.indexing_trigger {
        row("indexing_trigger", GridCell::toned(trigger, Tone::Warn));
    }
    if let Some(input) = &detail.input_type {
        row("input_type", GridCell::plain(input));
    }
    if let Some(access) = &detail.access_type {
        row("access_type", GridCell::plain(access));
    }
    if let Some(name) = &detail.credential_name {
        row("credential", GridCell::plain(name));
    } else if let Some(id) = detail.credential_id {
        row("credential", GridCell::plain(format!("#{id}")));
    }
    row(
        "created",
        GridCell::plain(timestamp_opt(detail.time_created.as_ref(), relative)),
    );
    row(
        "updated",
        GridCell::plain(timestamp_opt(detail.time_updated.as_ref(), relative)),
    );
    row(
        "last_pruned",
        GridCell::plain(timestamp_opt(detail.last_pruned.as_ref(), relative)),
    );

    let a = &detail.attempts;
    row(
        "attempts",
        GridCell::plain(format!(
            "{} success · {} failed · {} canceled · {} running · {} not started · {} partial",
            a.success, a.failed, a.canceled, a.in_progress, a.not_started, a.completed_with_errors
        )),
    );

    if let Some(last) = &s.last_attempt {
        let status = last.status.as_deref().unwrap_or("—");
        row(
            "last_attempt",
            GridCell::toned(
                format!(
                    "#{} {status} {}",
                    last.id.map(|i| i.to_string()).unwrap_or_else(|| "—".into()),
                    timestamp_opt(last.time_updated.as_ref(), relative)
                ),
                status_tone(status),
            ),
        );
        if let Some(msg) = &last.error_msg {
            row("last_error", GridCell::toned(msg.trim(), Tone::Dim));
        }
    }

    if let Some(config) = &detail.connector_specific_config {
        row(
            "config",
            GridCell::plain(
                serde_json::to_string_pretty(config).unwrap_or_else(|_| config.to_string()),
            ),
        );
    }

    if let Some(history) = &detail.history {
        let sparkline: String = history
            .iter()
            .map(|p| {
                spark(
                    p.docs_added,
                    history.iter().map(|h| h.docs_added).max().unwrap_or(0),
                )
            })
            .collect();
        let total: i64 = history.iter().map(|h| h.docs_added).sum();
        row(
            "history",
            GridCell::plain(format!(
                "{sparkline}  {} docs over {} days",
                thousands(total),
                history.len()
            )),
        );
    }

    grid
}

fn spark(value: i64, max: i64) -> char {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if max <= 0 {
        return '▁';
    }
    let index = ((value as f64 / max as f64) * (BARS.len() - 1) as f64).round() as usize;
    BARS[index.min(BARS.len() - 1)]
}

// ---------------------------------------------------------------------------
// Index attempts and their errors
// ---------------------------------------------------------------------------

pub const ATTEMPT_COLUMNS: &[ColSpec] = &[
    ColSpec::new("id"),
    ColSpec::new("connector"),
    ColSpec::new("status"),
    ColSpec::new("docs"),
    ColSpec::new("batches"),
    ColSpec::new("rate"),
    ColSpec::new("updated"),
    ColSpec::wide("cc_pair"),
    ColSpec::wide("started"),
    ColSpec::wide("heartbeat"),
    ColSpec::wide("from_beginning"),
    ColSpec::wide("error"),
];

pub fn attempts(ctx: &Ctx, items: &[IndexAttemptItem]) -> CliResult<Grid> {
    let chosen = select_columns(ATTEMPT_COLUMNS, ctx.out.wide, ctx.out.columns.as_deref())
        .map_err(CliError::Usage)?;
    let relative = ctx.relative_time();
    let mut grid = Grid::new(chosen.iter().map(|c| header(c)).collect());

    for a in items {
        let row = chosen
            .iter()
            .map(|col| match *col {
                "id" => GridCell::plain(a.id.to_string()),
                "connector" => GridCell::plain(a.connector_name.as_deref().unwrap_or("—")),
                "status" => {
                    // `stalled` and `parked` change what the status means, so
                    // they ride in the same cell rather than a column nobody
                    // asked for.
                    let mut text = a.status.clone();
                    if a.stalled {
                        text.push_str(" ⚠stalled");
                    }
                    if a.parked {
                        text.push_str(" ⏸parked");
                    }
                    let tone = if a.stalled {
                        Tone::Error
                    } else if a.parked {
                        Tone::Warn
                    } else {
                        status_tone(&a.status)
                    };
                    GridCell::toned(text, tone)
                }
                "docs" => GridCell::plain(format!(
                    "{} new / {} total",
                    a.new_docs_indexed.unwrap_or(0),
                    a.total_docs_indexed.unwrap_or(0)
                )),
                "batches" => GridCell::plain(match a.total_batches {
                    Some(total) => format!("{}/{}", a.completed_batches, total),
                    None => a.completed_batches.to_string(),
                }),
                "rate" => match a.pages_per_min {
                    Some(rate) => GridCell::plain(format!("{rate:.1}/min")),
                    None => GridCell::toned("—", Tone::Dim),
                },
                "updated" => GridCell::plain(timestamp(&a.time_updated, relative)),
                "cc_pair" => GridCell::plain(a.cc_pair_id.to_string()),
                "started" => GridCell::plain(timestamp_opt(a.time_started.as_ref(), relative)),
                "heartbeat" => {
                    GridCell::plain(timestamp_opt(a.last_heartbeat_time.as_ref(), relative))
                }
                "from_beginning" => bool_cell(a.from_beginning, Tone::Info),
                "error" => match &a.error_msg {
                    Some(msg) => GridCell::toned(msg.trim(), Tone::Dim),
                    None => GridCell::plain(""),
                },
                other => GridCell::plain(format!("<{other}?>")),
            })
            .collect();
        grid.push(row);
    }
    Ok(grid)
}

pub const ATTEMPT_ERROR_COLUMNS: &[ColSpec] = &[
    ColSpec::new("#"),
    ColSpec::new("when"),
    ColSpec::new("document"),
    ColSpec::new("message"),
    ColSpec::wide("id"),
    ColSpec::wide("attempt"),
    ColSpec::wide("type"),
    ColSpec::wide("resolved"),
];

pub fn attempt_errors(
    ctx: &Ctx,
    items: &[IndexAttemptError],
    first_handle: Option<usize>,
) -> CliResult<Grid> {
    let mut chosen = select_columns(
        ATTEMPT_ERROR_COLUMNS,
        ctx.out.wide,
        ctx.out.columns.as_deref(),
    )
    .map_err(CliError::Usage)?;
    if first_handle.is_none() && ctx.out.columns.is_none() {
        chosen.retain(|c| *c != "#");
    }
    let relative = ctx.relative_time();
    let mut grid = Grid::new(chosen.iter().map(|c| header(c)).collect());

    for (offset, e) in items.iter().enumerate() {
        // A handle is only printed for rows that have a document to hand to
        // `page view`; an error with no document id would otherwise show an
        // `@N` that resolves to nothing.
        let handle = first_handle
            .filter(|_| e.document_id.is_some())
            .map(|first| first + offset);
        let row = chosen
            .iter()
            .map(|col| match *col {
                "#" => GridCell::toned(
                    handle.map(|n| format!("@{n}")).unwrap_or_default(),
                    Tone::Dim,
                ),
                "when" => GridCell::plain(timestamp(&e.time_created, relative)),
                "document" => GridCell::plain(
                    e.document_link
                        .as_deref()
                        .or(e.document_id.as_deref())
                        .unwrap_or("—"),
                ),
                "message" => GridCell::toned(e.failure_message.trim(), Tone::Error),
                "id" => GridCell::plain(e.id.to_string()),
                "attempt" => GridCell::plain(e.index_attempt_id.to_string()),
                "type" => GridCell::plain(e.error_type.as_deref().unwrap_or("—")),
                "resolved" => bool_cell(e.is_resolved, Tone::Ok),
                other => GridCell::plain(format!("<{other}?>")),
            })
            .collect();
        grid.push(row);
    }
    Ok(grid)
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub fn stats_overview(ctx: &Ctx, stats: &StatsOverview) -> Grid {
    let color = ctx.out.color;
    let mut grid = Grid::new(vec!["METRIC".into(), "VALUE".into()]);
    let mut row = |k: &str, v: GridCell| grid.push(vec![GridCell::plain(k), v]);

    row(
        "documents",
        GridCell::plain(format!(
            "{}{}",
            thousands(stats.documents),
            if stats.documents_exact {
                ""
            } else {
                " (estimate)"
            }
        )),
    );
    row(
        "chunks",
        GridCell::plain(stats.chunks.map(thousands).unwrap_or_else(|| "—".into())),
    );

    let c = &stats.connectors;
    row(
        "connectors",
        GridCell::plain(format!(
            "{} total · {} active · {} paused · {} initial · {} parked{}{}",
            c.total,
            c.active,
            c.paused,
            c.initial_indexing,
            c.parked,
            if c.deleting > 0 {
                format!(" · {} deleting", c.deleting)
            } else {
                String::new()
            },
            if c.invalid > 0 {
                format!(" · {} invalid", c.invalid)
            } else {
                String::new()
            },
        )),
    );

    let i = &stats.index;
    row("index", GridCell::plain(&i.name));
    row(
        "index size",
        GridCell::plain(format!(
            "{}{}",
            i.size_bytes.map(bytes).unwrap_or_else(|| "—".into()),
            i.deleted_docs
                .filter(|d| *d > 0)
                .map(|d| format!(" · {} deleted docs", thousands(d)))
                .unwrap_or_default()
        )),
    );
    if let Some(pct) = i.disk_used_pct {
        // The 400 GB index has tripped the flood-stage watermark before, which
        // is why disk is a first-class line rather than a footnote.
        let tone = if i.read_only || pct >= 90.0 {
            Tone::Error
        } else if pct >= 80.0 {
            Tone::Warn
        } else {
            Tone::Ok
        };
        row(
            "disk",
            GridCell::toned(
                format!(
                    "{pct:.0}% used · {} free{}",
                    i.disk_available_bytes
                        .map(bytes)
                        .unwrap_or_else(|| "—".into()),
                    if i.read_only {
                        "  ⚠ INDEX IS READ-ONLY (flood-stage watermark)"
                    } else {
                        ""
                    }
                ),
                tone,
            ),
        );
    }
    if let Some(status) = &i.cluster_status {
        let tone = match status.as_str() {
            "green" => Tone::Ok,
            "yellow" => Tone::Warn,
            _ => Tone::Error,
        };
        row("cluster", GridCell::toned(status, tone));
    }
    row(
        "embedding",
        GridCell::plain(format!(
            "{} ({}d)",
            stats.embedding.model, stats.embedding.dim
        )),
    );

    let cr = &stats.crawl;
    row(
        "crawl",
        GridCell::toned(
            format!(
                "{} docs/15m · {} docs/24h · {} attempts running{}",
                thousands(cr.docs_last_15m),
                thousands(cr.docs_last_24h),
                cr.attempts_in_progress,
                if cr.attempts_stalled > 0 {
                    format!(" · {} STALLED", cr.attempts_stalled)
                } else {
                    String::new()
                }
            ),
            if cr.attempts_stalled > 0 {
                Tone::Error
            } else {
                Tone::Plain
            },
        ),
    );

    let a = &stats.attempts;
    row(
        "attempts",
        GridCell::plain(format!(
            "{} success · {} failed · {} canceled · {} running",
            a.success, a.failed, a.canceled, a.in_progress
        )),
    );

    let _ = color;
    grid
}

pub fn stats_sources(ctx: &Ctx, sources: &[SourceStat]) -> Grid {
    let _ = ctx;
    let mut grid = Grid::new(vec![
        "SOURCE".into(),
        "CONNECTORS".into(),
        "DOCUMENTS".into(),
        "CHUNKS".into(),
    ]);
    for s in sources {
        grid.push(vec![
            GridCell::plain(&s.source),
            GridCell::plain(s.connectors.to_string()),
            GridCell::plain(thousands(s.documents)),
            GridCell::plain(s.chunks.map(thousands).unwrap_or_else(|| "—".into())),
        ]);
    }
    grid
}

pub fn stats_top_connectors(ctx: &Ctx, items: &[TopConnector]) -> Grid {
    let relative = ctx.relative_time();
    let mut grid = Grid::new(vec![
        "NAME".into(),
        "SOURCE".into(),
        "STATUS".into(),
        "DOCUMENTS".into(),
        "LAST SUCCESS".into(),
    ]);
    for c in items {
        grid.push(vec![
            GridCell::plain(&c.name),
            GridCell::plain(&c.source),
            GridCell::toned(&c.status, status_tone(&c.status)),
            GridCell::plain(thousands(c.doc_count)),
            GridCell::plain(timestamp_opt(
                c.last_successful_index_time.as_ref(),
                relative,
            )),
        ]);
    }
    grid
}

pub fn stats_timeline(ctx: &Ctx, timeline: &TimelineResponse) -> Grid {
    let relative = ctx.relative_time();
    let max = timeline.items.iter().map(|b| b.docs).max().unwrap_or(0);
    let mut grid = Grid::new(vec!["BUCKET".into(), "DOCUMENTS".into(), "".into()]);
    for b in &timeline.items {
        // A bar chart is what a timeline is for; a column of numbers is not.
        let width = if max > 0 {
            ((b.docs as f64 / max as f64) * 40.0).round() as usize
        } else {
            0
        };
        grid.push(vec![
            GridCell::plain(timestamp(&b.bucket, relative)),
            GridCell::plain(thousands(b.docs)),
            GridCell::toned("█".repeat(width), Tone::Info),
        ]);
    }
    grid
}

// ---------------------------------------------------------------------------

/// Column headers are upper-cased for the table and left as-is for CSV keys.
fn header(name: &str) -> String {
    match name {
        "#" => "#".to_string(),
        other => other.to_ascii_uppercase().replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            api: crate::api::ApiClient::new("http://127.0.0.1:8080", None, false).unwrap(),
            out: crate::output::Out::default(),
            interaction: crate::prompt::Interaction::default(),
            cfg: crate::config::resolve_with(
                &crate::config::Overrides::default(),
                std::path::PathBuf::from("/nonexistent/config.toml"),
                crate::config::ConfigFile::default(),
                &crate::config::EnvVars::default(),
            )
            .unwrap(),
        }
    }

    fn item(chunk_count: Option<i32>) -> PageListItem {
        PageListItem {
            id: "https://example.com/a".into(),
            semantic_id: "A page".into(),
            link: Some("https://example.com/a".into()),
            updated_at: "2026-07-20T00:00:00Z".parse().unwrap(),
            doc_updated_at: None,
            last_modified: "2026-07-20T00:00:00Z".parse().unwrap(),
            chunk_count,
            boost: 0,
            hidden: false,
            connector_id: Some(4),
            connector_name: Some("tildes".into()),
            connector_source: Some("WEB".into()),
            metadata: None,
        }
    }

    #[test]
    fn an_uncounted_document_is_not_rendered_as_zero_chunks() {
        // `chunk_count: null` means Onyx has not counted it yet, which the
        // --stubs filter deliberately excludes. Showing 0 would be a lie.
        assert_eq!(chunk_cell(None).text, "—");
        assert_eq!(chunk_cell(Some(0)).text, "0");
        assert_eq!(chunk_cell(Some(34)).text, "34");
    }

    #[test]
    fn the_default_page_columns_are_the_documented_set() {
        let grid = pages(&ctx(), &[item(Some(3))], Some(1)).unwrap();
        assert_eq!(
            grid.headers,
            vec!["#", "TITLE", "CONNECTOR", "CHUNKS", "UPDATED", "URL"]
        );
        assert_eq!(grid.rows[0][0].text, "@1");
        assert_eq!(grid.rows[0][1].text, "A page");
    }

    #[test]
    fn handles_are_omitted_when_no_list_is_being_recorded() {
        // `--all` streams 1.6 M rows; numbering them would be meaningless.
        let grid = pages(&ctx(), &[item(Some(3))], None).unwrap();
        assert!(!grid.headers.contains(&"#".to_string()));
    }

    #[test]
    fn handles_continue_across_pages_rather_than_restarting_at_one() {
        let items = vec![item(Some(1)), item(Some(2))];
        let grid = pages(&ctx(), &items, Some(51)).unwrap();
        assert_eq!(grid.rows[0][0].text, "@51");
        assert_eq!(grid.rows[1][0].text, "@52");
    }

    #[test]
    fn wide_adds_the_full_column_set() {
        let mut ctx = ctx();
        ctx.out.wide = true;
        let grid = pages(&ctx, &[item(Some(3))], Some(1)).unwrap();
        assert!(grid.headers.contains(&"ID".to_string()));
        assert!(grid.headers.contains(&"BOOST".to_string()));
        assert!(grid.headers.contains(&"DOC UPDATED".to_string()));
    }

    #[test]
    fn explicit_columns_win_over_the_default_and_keep_their_order() {
        let mut ctx = ctx();
        ctx.out.columns = Some("url,title".into());
        let grid = pages(&ctx, &[item(Some(3))], Some(1)).unwrap();
        assert_eq!(grid.headers, vec!["URL", "TITLE"]);
        assert_eq!(grid.rows[0][0].text, "https://example.com/a");
    }

    #[test]
    fn an_unknown_column_is_a_usage_error() {
        let mut ctx = ctx();
        ctx.out.columns = Some("titel".into());
        let err = pages(&ctx, &[item(None)], Some(1)).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::exit::USAGE);
    }

    fn detail(recrawl: bool, pg_row: bool) -> PageDetail {
        PageDetail {
            item: item(Some(14)),
            primary_owners: None,
            secondary_owners: None,
            content_hash: None,
            from_ingestion_api: Some(false),
            last_synced: None,
            cc_pair_id: Some(5),
            cc_pair_status: Some("ACTIVE".into()),
            tags: vec![TagKv {
                key: "author".into(),
                value: "kant".into(),
            }],
            pg_row,
            recrawl_risk: recrawl,
        }
    }

    #[test]
    fn the_detail_card_surfaces_recrawl_risk_and_orphaned_chunks() {
        let rendered = page_detail(&ctx(), &detail(true, false));
        let text: String = rendered
            .rows
            .iter()
            .map(|r| format!("{}={}", r[0].text, r[1].text))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("recrawl_risk"), "{text}");
        assert!(text.contains("likely to be undone"), "{text}");
        assert!(text.contains("orphaned chunks"), "{text}");
        assert!(text.contains("author=kant"), "{text}");
    }

    #[test]
    fn a_healthy_document_does_not_carry_the_warning_rows() {
        let rendered = page_detail(&ctx(), &detail(false, true));
        let fields: Vec<&str> = rendered.rows.iter().map(|r| r[0].text.as_str()).collect();
        assert!(!fields.contains(&"recrawl_risk"));
        assert!(!fields.contains(&"pg_row"));
    }

    fn connector(parked: bool, errors: bool) -> ConnectorSummary {
        ConnectorSummary {
            connector_id: 4,
            cc_pair_id: 5,
            name: "tildes".into(),
            source: "WEB".into(),
            status: "PAUSED".into(),
            parked,
            in_repeated_error_state: errors,
            doc_count: 105_666,
            last_successful_index_time: None,
            refresh_freq_secs: Some(2_592_000),
            indexing_trigger: None,
            last_attempt: None,
        }
    }

    #[test]
    fn parked_and_repeated_errors_ride_in_the_status_cell() {
        assert_eq!(
            connector_status_cell(&connector(false, false)).text,
            "PAUSED"
        );
        assert!(connector_status_cell(&connector(true, false))
            .text
            .contains("parked"));
        let both = connector_status_cell(&connector(true, true));
        assert!(both.text.contains("parked") && both.text.contains("errors"));
        assert_eq!(both.tone, Tone::Error);
    }

    #[test]
    fn connector_document_counts_are_grouped_not_raw() {
        let grid = connectors(&ctx(), &[connector(false, false)], Some(1)).unwrap();
        let docs = grid.headers.iter().position(|h| h == "DOCS").unwrap();
        assert_eq!(grid.rows[0][docs].text, "105,666");
    }

    #[test]
    fn sparkline_buckets_scale_against_the_maximum() {
        assert_eq!(spark(0, 100), '▁');
        assert_eq!(spark(100, 100), '█');
        // A flat-zero history must not divide by zero.
        assert_eq!(spark(0, 0), '▁');
    }

    #[test]
    fn a_read_only_index_is_shouted_about_rather_than_shown_as_a_percentage() {
        let mut stats = overview_fixture();
        stats.index.read_only = true;
        let grid = stats_overview(&ctx(), &stats);
        let text = grid
            .rows
            .iter()
            .map(|r| r[1].text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("READ-ONLY"), "{text}");
    }

    #[test]
    fn an_estimated_document_total_is_labelled_as_one() {
        let mut stats = overview_fixture();
        stats.documents_exact = false;
        let grid = stats_overview(&ctx(), &stats);
        assert!(grid.rows[0][1].text.contains("estimate"));
    }

    fn overview_fixture() -> StatsOverview {
        StatsOverview {
            documents: 1_669_976,
            documents_exact: true,
            chunks: Some(10_078_452),
            connectors: ConnectorStatusCounts {
                total: 332,
                active: 41,
                paused: 278,
                initial_indexing: 13,
                deleting: 0,
                invalid: 0,
                parked: 94,
            },
            index: IndexStats {
                name: "danswer_chunk_snowflake_arctic_embed_m".into(),
                size_bytes: Some(401_889_503_458),
                docs: Some(10_078_452),
                deleted_docs: Some(351_384),
                disk_used_pct: Some(54.0),
                disk_total_bytes: Some(844_367_142_912),
                disk_available_bytes: Some(380_953_731_072),
                read_only: false,
                cluster_status: Some("green".into()),
            },
            embedding: EmbeddingInfo {
                model: "snowflake-arctic-embed:m".into(),
                dim: 768,
            },
            crawl: CrawlStats {
                docs_last_15m: 17,
                docs_last_24h: 225_430,
                attempts_in_progress: 10,
                attempts_stalled: 0,
            },
            attempts: AttemptAggregates {
                success: 3411,
                failed: 1056,
                canceled: 3950,
                in_progress: 10,
                not_started: 5,
                completed_with_errors: 37,
                other: 0,
            },
        }
    }
}
