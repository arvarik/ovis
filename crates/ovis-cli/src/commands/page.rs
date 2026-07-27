//! `ovis page …` — the workhorse noun.

use ovis_core::api_types::{
    BatchDeleteResponse, DeleteOutcome, ListResponse, PageDetail, PageListItem, PagePatch,
};

use crate::api::QueryBuilder;
use crate::cli::PageListArgs;
use crate::ctx::Ctx;
use crate::error::{usage, CliError, CliResult};
use crate::handles::{self, HandleItem, HandleKind};
use crate::output::style::Tone;
use crate::output::{thousands, Format};
use crate::render;
use crate::resolve;

/// Deep offset paging is refused by the server past 50,000 rows; say so before
/// the round-trip rather than after.
const MAX_OFFSET_DEPTH: i64 = 50_000;

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

pub async fn list(ctx: &Ctx, args: &PageListArgs) -> CliResult<()> {
    // Usage mistakes are caught before any request, so a typo does not report
    // itself as a connection failure.
    render::validate_columns(ctx, render::PAGE_COLUMNS)?;

    let mut query = QueryBuilder::new();
    let mut described = Vec::new();

    if let Some(q) = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        query.push("search", q);
        described.push(format!("{q:?}"));
    }

    // A number could be a connector_id or a cc-pair id; resolving it also gives
    // us the name for the footer, and catches a typo before the list query runs.
    if let Some(reference) = &args.connector {
        let resolved = resolve::connector(ctx, reference).await?;
        query.push("connector_id", resolved.connector_id());
        described.push(format!("connector {}", resolved.name()));
    }

    if let Some(source) = &args.source {
        query.push("source", resolve::normalise_source(source));
    }

    let (chunk_min, chunk_max) = chunk_bounds(args)?;
    query.push_opt("chunk_min", chunk_min);
    query.push_opt("chunk_max", chunk_max);

    if args.hidden {
        query.push("hidden", true);
    } else if args.visible {
        query.push("hidden", false);
    }

    if let Some(since) = &args.since {
        let ts = resolve::parse_when(since).map_err(CliError::Usage)?;
        query.push("updated_after", ts.to_rfc3339());
    }
    if let Some(until) = &args.until {
        let ts = resolve::parse_when(until).map_err(CliError::Usage)?;
        query.push("updated_before", ts.to_rfc3339());
    }

    if let Some(sort) = &args.sort {
        query.push("sort", resolve::parse_sort(sort).map_err(CliError::Usage)?);
    }

    if args.all {
        return stream_all(ctx, &mut query, args).await;
    }

    let limit = args.limit.unwrap_or_else(|| ctx.out.default_limit()).max(1);
    query.push("limit", limit);

    let page = args.page.unwrap_or(1).max(1);
    if let Some(cursor) = &args.cursor {
        query.push("cursor", cursor);
    } else {
        if (page - 1) * limit > MAX_OFFSET_DEPTH {
            return usage(format!(
                "page {page} at {limit} rows each is past the {} row offset limit; use the \
                 --cursor token from the previous page instead",
                thousands(MAX_OFFSET_DEPTH)
            ));
        }
        query.push("page", page);
    }

    let response = ctx.api.pages(&query.build()).await?;

    if args.pick {
        return pick_one(ctx, &response.items).await;
    }

    // A cursor gives no absolute position, so numbering restarts at 1 for it.
    let first_handle = match args.cursor {
        Some(_) => 1,
        None => ((page - 1) * limit + 1) as usize,
    };

    emit_pages(ctx, &response, first_handle, &describe_command(args))?;
    list_footer(ctx, &response, first_handle, args, &described);
    Ok(())
}

fn chunk_bounds(args: &PageListArgs) -> CliResult<(Option<i32>, Option<i32>)> {
    if args.stubs {
        // chunk_count == 0 exactly. Rows with a null count are deliberately
        // excluded by the server: "not counted yet" is not "no chunks".
        return Ok((Some(0), Some(0)));
    }
    if args.heavy {
        return Ok((Some(11), None));
    }
    match &args.chunks {
        Some(raw) => resolve::parse_chunk_range(raw).map_err(CliError::Usage),
        None => Ok((None, None)),
    }
}

/// The exact command that reproduces this list, for the staleness message a
/// `@N` handle prints an hour later.
fn describe_command(args: &PageListArgs) -> String {
    let mut parts = vec!["ovis page list".to_string()];
    if let Some(q) = &args.query {
        parts.push(format!("{q:?}"));
    }
    if let Some(c) = &args.connector {
        parts.push(format!("-c {c}"));
    }
    if let Some(s) = &args.source {
        parts.push(format!("-s {s}"));
    }
    if args.stubs {
        parts.push("--stubs".into());
    }
    if args.heavy {
        parts.push("--heavy".into());
    }
    parts.join(" ")
}

fn emit_pages(
    ctx: &Ctx,
    response: &ListResponse<PageListItem>,
    first_handle: usize,
    command: &str,
) -> CliResult<()> {
    match ctx.out.format {
        // The wire struct verbatim: `--format json` is the API response.
        Format::Json => ctx.out.json(response)?,
        Format::Yaml => ctx.out.yaml(response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let grid = render::pages(ctx, &response.items, Some(first_handle))?;
            ctx.out.grid(&grid)?;
        }
    }

    handles::save(
        HandleKind::Page,
        command,
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
    Ok(())
}

/// The footer always teaches the next step (`05_PAGE_NAVIGATION_UX.md` §1).
fn list_footer(
    ctx: &Ctx,
    response: &ListResponse<PageListItem>,
    first_handle: usize,
    args: &PageListArgs,
    described: &[String],
) {
    if response.items.is_empty() {
        ctx.out.footer(format!(
            "no pages matched{}",
            if described.is_empty() {
                String::new()
            } else {
                format!(" {}", described.join(" · "))
            }
        ));
        return;
    }

    let last = first_handle + response.items.len() - 1;
    let total = if response.total_exact {
        thousands(response.total)
    } else {
        format!("~{}", thousands(response.total))
    };

    let mut footer = format!(
        "{}–{} of {}",
        thousands(first_handle as i64),
        thousands(last as i64),
        total
    );
    if let Some(page) = response.page {
        footer.push_str(&format!(" · page {page}"));
    }

    if response.has_more {
        let next = match (&response.next_cursor, response.page) {
            // Offset paging is cheap and readable inside the depth limit; past
            // it the cursor is the only thing that works.
            (_, Some(page)) if (page * response.limit) <= MAX_OFFSET_DEPTH => {
                format!("ovis page list --page {}", page + 1)
            }
            (Some(cursor), _) => format!("ovis page list --cursor {cursor}"),
            _ => "ovis page list --all -o ndjson".to_string(),
        };
        footer.push_str(&format!(" · next: {next}"));
    }
    ctx.out.footer(footer);

    if ctx.out.format == Format::Table && ctx.out.stdout_tty {
        ctx.out
            .footer("view: ovis page view @N · text: ovis page text @N · open: ovis page open @N");
        maybe_suggest_tui(ctx, args);
    }
}

/// After three consecutive refinements, mention the interactive mode — once a
/// day, and never when the user has turned hints off.
fn maybe_suggest_tui(ctx: &Ctx, args: &PageListArgs) {
    if !ctx.out.hints {
        return;
    }
    let refining = args.query.is_some()
        || args.connector.is_some()
        || args.source.is_some()
        || args.stubs
        || args.heavy
        || args.chunks.is_some()
        || args.since.is_some();
    if !refining {
        return;
    }

    let marker = crate::config::state_dir().join("hint-tui");
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    if std::fs::read_to_string(&marker).is_ok_and(|seen| seen.trim() == today) {
        return;
    }
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, &today);

    let scope = args
        .query
        .as_deref()
        .map(|q| format!(" --query {q}"))
        .unwrap_or_default();
    ctx.out.hint(format!(
        "interactive browsing: ovis tui{scope}  (silence with `ovis config set ui.hints false`)"
    ));
}

/// Larger than any conceivable corpus, so the server's own `max_stream_limit`
/// is the only thing that bounds an unqualified `--all`.
///
/// Without this the stream handler's *default* limit of 1,000 applies, and
/// `--all` over a 105,666-document connector quietly emits 1,000 rows and calls
/// it done. That is the same class of defect as the sample-data fallback: a
/// wrong answer that looks like a right one.
const STREAM_EVERYTHING: i64 = 1_000_000_000;

/// `--all`: stream every matching row over SSE rather than paging by hand.
async fn stream_all(ctx: &Ctx, query: &mut QueryBuilder, args: &PageListArgs) -> CliResult<()> {
    if matches!(ctx.out.format, Format::Yaml) {
        // YAML is one document; producing it would mean buffering 1.6 M rows.
        return usage(
            "--all cannot render YAML, which would have to buffer the whole result set. Use \
             -o ndjson (or csv) to stream.",
        );
    }

    // What the filter actually matches, so an incomplete stream can be
    // recognised as incomplete rather than reported as the whole answer.
    let expected = {
        let mut probe = query.clone();
        probe.push("limit", 1);
        let head = ctx.api.pages(&probe.build()).await?;
        (head.total, head.total_exact)
    };

    query.push("limit", args.limit.unwrap_or(STREAM_EVERYTHING));
    let response = ctx.api.stream(&query.build()).await?;

    let progress = if ctx.out.stderr_tty && !ctx.out.quiet {
        let bar = indicatif::ProgressBar::new(expected.0.max(0) as u64);
        bar.set_style(
            indicatif::ProgressStyle::with_template(
                "{spinner} {pos}/{len} rows · {elapsed_precise}",
            )
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_spinner()),
        );
        Some(bar)
    } else {
        None
    };

    // Writing straight through as rows arrive is the point: the memory profile
    // has to be flat whether the answer is 50 rows or 1.6 million.
    let mut writer = StreamWriter::new(ctx);
    let emitted = crate::sse::consume(response, |data| {
        if let Some(bar) = &progress {
            bar.inc(1);
        }
        writer.row(data)
    })
    .await;

    if let Some(bar) = progress {
        bar.finish_and_clear();
    }
    let emitted = emitted?;
    writer.finish()?;

    report_stream_completeness(ctx, emitted, expected, args.limit)
}

/// Say plainly whether `--all` actually meant all.
fn report_stream_completeness(
    ctx: &Ctx,
    emitted: u64,
    (total, total_exact): (i64, bool),
    user_limit: Option<i64>,
) -> CliResult<()> {
    let streamed = thousands(emitted as i64);

    if (emitted as i64) >= total {
        ctx.out.footer(format!("{streamed} rows streamed"));
        return Ok(());
    }

    // The user asked for at most N and got N: their own bound, not a surprise.
    if user_limit.is_some_and(|limit| emitted as i64 >= limit) {
        ctx.out.footer(format!(
            "{streamed} rows streamed (your --limit; {} match the filter)",
            thousands(total)
        ));
        return Ok(());
    }

    // Otherwise the server's OVIS_MAX_STREAM_LIMIT cut the stream short, and
    // the output is a partial answer. Exit 11 so a pipeline notices.
    ctx.out.warn(format!(
        "the stream stopped at {streamed} rows but {}{} match the filter — the server's \
         OVIS_MAX_STREAM_LIMIT capped it",
        if total_exact { "" } else { "about " },
        thousands(total)
    ));
    ctx.out
        .hint("raise OVIS_MAX_STREAM_LIMIT on the server, or page through with --page / --cursor");
    Err(CliError::PartialFailure(format!(
        "--all returned {streamed} of {} matching rows",
        thousands(total)
    )))
}

/// Emits streamed rows in the requested format without ever holding more than
/// one row.
struct StreamWriter<'a> {
    ctx: &'a Ctx,
    header_written: bool,
    count: u64,
    buffer: String,
    /// Decided from the first row's columns and then held constant.
    widths: Option<Vec<usize>>,
}

impl<'a> StreamWriter<'a> {
    fn new(ctx: &'a Ctx) -> Self {
        Self {
            ctx,
            header_written: false,
            count: 0,
            buffer: String::new(),
            widths: None,
        }
    }

    fn row(&mut self, data: &str) -> CliResult<()> {
        match self.ctx.out.format {
            Format::Ndjson => {
                self.buffer.push_str(data);
                self.buffer.push('\n');
            }
            Format::Json => {
                // Hand-assembled so the envelope stays a single valid document
                // without the items ever being collected.
                if !self.header_written {
                    self.buffer.push_str("{\"items\":[");
                    self.header_written = true;
                } else {
                    self.buffer.push(',');
                }
                self.buffer.push_str(data);
            }
            Format::Csv | Format::Table => {
                let item: PageListItem = serde_json::from_str(data)?;
                let grid = render::pages(self.ctx, std::slice::from_ref(&item), None)?;
                let headers = !self.header_written && !self.ctx.out.no_headers;
                let text = match self.ctx.out.format {
                    Format::Csv => crate::output::table::render_csv(&grid, headers)
                        .map_err(|e| CliError::Other(anyhow::anyhow!("cannot render CSV: {e}")))?,
                    // Fixed widths, decided once: rows are rendered one at a
                    // time as they arrive, so per-row widths would pad every
                    // line differently and nothing would line up.
                    _ => {
                        let widths = self
                            .widths
                            .get_or_insert_with(|| stream_column_widths(&grid.headers));
                        crate::output::table::render_plain_with(&grid, headers, widths)
                    }
                };
                self.header_written = true;
                self.buffer.push_str(&text);
            }
            Format::Yaml => unreachable!("--all refuses YAML before the stream opens"),
        }
        self.count += 1;
        // Flush in blocks so a downstream `head` sees output promptly without
        // one write syscall per row.
        if self.buffer.len() > 32 * 1024 {
            let chunk = std::mem::take(&mut self.buffer);
            self.ctx.out.print(chunk)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> CliResult<()> {
        if self.ctx.out.format == Format::Json {
            if !self.header_written {
                self.buffer.push_str("{\"items\":[");
            }
            self.buffer.push_str(&format!(
                "],\"total\":{},\"total_exact\":true,\"page\":null,\"limit\":{},\
                 \"next_cursor\":null,\"has_more\":false}}",
                self.count, self.count
            ));
        }
        if !self.buffer.is_empty() {
            let chunk = std::mem::take(&mut self.buffer);
            self.ctx.out.print(chunk)?;
        }
        Ok(())
    }
}

/// Column widths for a streamed table, by column name.
///
/// Generous enough for real values without being so wide that a terminal wraps:
/// titles and URLs are the long ones, and the URL is last so it can overflow
/// without disturbing anything to its left.
fn stream_column_widths(headers: &[String]) -> Vec<usize> {
    headers
        .iter()
        .map(|header| match header.as_str() {
            "TITLE" => 60,
            "URL" | "ID" => 0,
            "CONNECTOR" => 20,
            "CHUNKS" | "BOOST" => 6,
            "UPDATED" | "LAST MODIFIED" | "DOC UPDATED" => 20,
            "SOURCE" => 8,
            "HIDDEN" => 6,
            other => other.len().max(8),
        })
        .collect()
}

/// `--pick`: choose one row inline, then show it.
async fn pick_one(ctx: &Ctx, items: &[PageListItem]) -> CliResult<()> {
    if items.is_empty() {
        ctx.out.footer("no pages matched");
        return Ok(());
    }
    let labels: Vec<String> = items
        .iter()
        .map(|i| format!("{}  {}", i.semantic_id, i.link.as_deref().unwrap_or(&i.id)))
        .collect();
    match crate::picker::pick("pick a page:", &labels)? {
        Some(index) => view(ctx, &items[index].id).await,
        None => {
            ctx.out.note("nothing picked");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// view / text / chunks / open
// ---------------------------------------------------------------------------

pub async fn view(ctx: &Ctx, reference: &str) -> CliResult<()> {
    let id = handles::resolve(reference, HandleKind::Page)?;
    let detail = ctx.api.page_detail(&id).await?;
    emit_detail(ctx, &detail)
}

fn emit_detail(ctx: &Ctx, detail: &PageDetail) -> CliResult<()> {
    match ctx.out.format {
        Format::Json => ctx.out.json(detail)?,
        Format::Yaml => ctx.out.yaml(detail)?,
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(detail))?,
        Format::Table | Format::Csv => {
            let grid = render::page_detail(ctx, detail);
            ctx.out.grid(&grid)?;
        }
    }

    if ctx.out.format == Format::Table {
        let chunks = detail
            .item
            .chunk_count
            .map(|c| format!("{c} chunk{}", if c == 1 { "" } else { "s" }))
            .unwrap_or_else(|| "chunk count not yet recorded".into());
        ctx.out.footer(format!(
            "{chunks} · text: ovis page text {id} · chunks: ovis page chunks {id}",
            id = shell_quote(&detail.item.id)
        ));
    }
    Ok(())
}

/// Ids are URLs; a naive copy-paste of one with a `?` in it would be mangled by
/// the shell, so the suggested command quotes it.
fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

pub async fn text(ctx: &Ctx, reference: &str, output: Option<&str>) -> CliResult<()> {
    let id = handles::resolve(reference, HandleKind::Page)?;
    let body = ctx.api.page_text(&id).await?;

    match output {
        Some(path) => {
            std::fs::write(path, &body)?;
            ctx.out
                .note(format!("wrote {} bytes to {path}", body.len()));
            Ok(())
        }
        None => ctx.out.page(&body),
    }
}

pub async fn chunks(
    ctx: &Ctx,
    reference: &str,
    limit: i64,
    after: Option<i64>,
    full: bool,
) -> CliResult<()> {
    render::validate_columns(ctx, render::CHUNK_COLUMNS)?;
    let id = handles::resolve(reference, HandleKind::Page)?;
    let mut query = QueryBuilder::new();
    query.push("limit", limit.max(1));
    query.push_opt("after", after);
    let response = ctx.api.page_chunks(&id, &query.build()).await?;

    match ctx.out.format {
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => ctx.out.ndjson(&response.items)?,
        Format::Table | Format::Csv => {
            let grid = render::chunks(ctx, &response.items, full)?;
            // A chunk dump is long by nature — the old one was unbounded and
            // unpaged.
            if ctx.out.format == Format::Table && full {
                ctx.out.page(&crate::output::table::render_boxed(
                    &grid,
                    ctx.out.color,
                    ctx.out.width,
                ))?;
            } else {
                ctx.out.grid(&grid)?;
            }
        }
    }

    if ctx.out.format == Format::Table {
        let mut footer = format!(
            "{} of {} chunks · {} ({}d)",
            response.items.len(),
            thousands(response.total_chunks),
            response.embedding_model,
            response.embedding_dim
        );
        if let Some(next) = response.next_after {
            footer.push_str(&format!(
                " · next: ovis page chunks {} --after {next}",
                shell_quote(&id)
            ));
        }
        ctx.out.footer(footer);
    }
    Ok(())
}

pub async fn open(ctx: &Ctx, reference: &str) -> CliResult<()> {
    let id = handles::resolve(reference, HandleKind::Page)?;
    let detail = ctx.api.page_detail(&id).await?;
    let target = detail.item.link.as_deref().unwrap_or(&detail.item.id);

    if !target.starts_with("http://") && !target.starts_with("https://") {
        return Err(CliError::Usage(format!(
            "'{target}' is not an http(s) link, so there is nothing to open"
        )));
    }
    open::that(target)
        .map_err(|e| CliError::Other(anyhow::anyhow!("cannot open a browser: {e}")))?;
    ctx.out.note(format!("opened {target}"));
    Ok(())
}

// ---------------------------------------------------------------------------
// edit
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn edit(
    ctx: &Ctx,
    reference: &str,
    title: Option<&str>,
    boost: Option<i32>,
    hide: bool,
    unhide: bool,
    meta: &[String],
) -> CliResult<()> {
    let id = handles::resolve(reference, HandleKind::Page)?;

    let metadata_merge = if meta.is_empty() {
        None
    } else {
        let mut map = serde_json::Map::new();
        for pair in meta {
            let Some((key, value)) = pair.split_once('=') else {
                return usage(format!("--meta expects KEY=VALUE, got {pair:?}"));
            };
            if key.trim().is_empty() {
                return usage(format!("--meta {pair:?} has an empty key"));
            }
            // A bare value is a string; anything that parses as JSON keeps its
            // type, so --meta 'tags=["a","b"]' does what it looks like.
            let parsed = serde_json::from_str(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
            map.insert(key.trim().to_string(), parsed);
        }
        Some(serde_json::Value::Object(map))
    };

    let patch = PagePatch {
        semantic_id: title.map(str::to_string),
        boost,
        hidden: if hide {
            Some(true)
        } else if unhide {
            Some(false)
        } else {
            None
        },
        metadata_merge,
    };

    if patch.is_empty() {
        return usage(
            "nothing to change; pass --title, --boost, --hide/--unhide or --meta KEY=VALUE",
        );
    }

    let response = ctx.api.page_patch(&id, &patch).await?;

    match ctx.out.format {
        Format::Json => ctx.out.json(&response)?,
        Format::Yaml => ctx.out.yaml(&response)?,
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(&response))?,
        Format::Table | Format::Csv => {
            let grid = render::page_detail(ctx, &response.detail);
            ctx.out.grid(&grid)?;
        }
    }

    // Report what actually happened rather than printing "updated".
    if ctx.out.format == Format::Table {
        let mut notes = Vec::new();
        if title.is_some() {
            notes.push(if response.index_synced {
                "title synced to the indexed chunks".to_string()
            } else {
                "title changed in Postgres but NOT synced to the index".to_string()
            });
        }
        if let Some(via) = &response.boost_hidden_via {
            notes.push(match via.as_str() {
                "onyx_api" => "boost/hidden applied through the Onyx API".to_string(),
                "direct_sql" => {
                    "boost/hidden written directly to Postgres (no Onyx token configured, so \
                     Onyx has not re-synced its index)"
                        .to_string()
                }
                other => format!("boost/hidden applied via {other}"),
            });
        }
        for note in notes {
            ctx.out.note(note);
        }
        if title.is_some() && !response.index_synced {
            ctx.out.warn(
                "the index still carries the old title; searches will show it until the \
                       next crawl",
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

pub async fn delete(ctx: &Ctx, ids: &[String], from_file: Option<&str>) -> CliResult<()> {
    let references = gather_ids(ids, from_file)?;
    if references.is_empty() {
        return usage("no document ids given");
    }
    let ids = handles::resolve_all(&references, HandleKind::Page)?;

    // Show what will actually happen — title, URL, chunk count, and whether the
    // connector will simply crawl it back.
    let details = fetch_details(ctx, &ids).await;
    confirm_delete(ctx, &ids, &details)?;

    if ids.len() == 1 {
        let outcome = ctx.api.page_delete(&ids[0]).await?;
        return report_single_delete(ctx, &ids[0], &outcome);
    }

    let (status, response) = ctx.api.pages_batch_delete(ids.clone()).await?;
    report_batch_delete(ctx, status, &response)
}

fn gather_ids(ids: &[String], from_file: Option<&str>) -> CliResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();

    for id in ids {
        if id == "-" {
            // `-` means stdin, which is why confirmations read /dev/tty.
            use std::io::BufRead;
            for line in std::io::stdin().lock().lines() {
                let line = line?;
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
        } else {
            out.push(id.clone());
        }
    }

    if let Some(path) = from_file {
        let text = std::fs::read_to_string(path)
            .map_err(|e| CliError::Other(anyhow::anyhow!("cannot read {path}: {e}")))?;
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                out.push(trimmed.to_string());
            }
        }
    }

    // The same id twice would make the batch report a failure for the second,
    // and `Vec::dedup` only collapses *adjacent* repeats — a list holding a, b,
    // a would still send a twice.
    let mut seen = std::collections::HashSet::new();
    out.retain(|id| seen.insert(id.clone()));
    Ok(out)
}

/// Best-effort detail for the confirmation summary. A document that cannot be
/// fetched still gets deleted — the summary just says less about it.
async fn fetch_details(ctx: &Ctx, ids: &[String]) -> Vec<Option<PageDetail>> {
    const SUMMARY_CAP: usize = 25;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids.iter().take(SUMMARY_CAP) {
        out.push(ctx.api.page_detail(id).await.ok());
    }
    out
}

fn confirm_delete(ctx: &Ctx, ids: &[String], details: &[Option<PageDetail>]) -> CliResult<()> {
    let chunk_total: i64 = details
        .iter()
        .flatten()
        .filter_map(|d| d.item.chunk_count)
        .map(i64::from)
        .sum();
    let at_risk = details.iter().flatten().filter(|d| d.recrawl_risk).count();

    if ctx.out.stderr_tty || !ctx.interaction.assume_yes {
        for (id, detail) in ids.iter().zip(details.iter()) {
            match detail {
                Some(d) => eprintln!(
                    "  {}  {}  {} chunks{}",
                    Tone::Bold.paint(&d.item.semantic_id, ctx.out.color && ctx.out.stderr_tty),
                    d.item.link.as_deref().unwrap_or(&d.item.id),
                    d.item
                        .chunk_count
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "?".into()),
                    if d.recrawl_risk {
                        "  [recrawl risk]"
                    } else {
                        ""
                    }
                ),
                None => eprintln!("  {id}  (could not be fetched)"),
            }
        }
        if ids.len() > details.len() {
            eprintln!("  … and {} more", ids.len() - details.len());
        }
    }

    if at_risk > 0 {
        ctx.out.warn(format!(
            "{at_risk} of these belong to a connector that is still active; the next scheduled \
             refresh will likely crawl them back"
        ));
    }

    let question = format!(
        "delete {} document{} ({} chunks)?",
        ids.len(),
        if ids.len() == 1 { "" } else { "s" },
        if details.len() < ids.len() {
            format!("at least {chunk_total}")
        } else {
            chunk_total.to_string()
        }
    );

    if crate::prompt::confirm(&question, ctx.interaction)? {
        Ok(())
    } else {
        Err(CliError::NeedsConfirmation(
            "cancelled; nothing was deleted".into(),
        ))
    }
}

fn report_single_delete(ctx: &Ctx, id: &str, outcome: &DeleteOutcome) -> CliResult<()> {
    match ctx.out.format {
        Format::Json => return ctx.out.json(outcome),
        Format::Yaml => return ctx.out.yaml(outcome),
        Format::Ndjson => return ctx.out.ndjson(std::slice::from_ref(outcome)),
        _ => {}
    }

    ctx.out.print(format!(
        "deleted {id}: {} row, {} chunk{} removed",
        if outcome.pg_deleted {
            "1 Postgres"
        } else {
            "no Postgres"
        },
        outcome.chunks_deleted,
        if outcome.chunks_deleted == 1 { "" } else { "s" }
    ))?;

    if outcome.index_cleanup_pending {
        ctx.out.warn(
            "Postgres committed but the index delete could not be confirmed; the id is queued \
             in ovis.pending_index_deletes and the server retries it",
        );
    }
    if outcome.recrawl_risk {
        ctx.out.warn(
            "this document's connector is still active, so the next scheduled refresh will \
             likely crawl it again",
        );
    }
    Ok(())
}

fn report_batch_delete(ctx: &Ctx, status: u16, response: &BatchDeleteResponse) -> CliResult<()> {
    match ctx.out.format {
        Format::Json => ctx.out.json(response)?,
        Format::Yaml => ctx.out.yaml(response)?,
        Format::Ndjson => ctx.out.ndjson(std::slice::from_ref(response))?,
        _ => {
            ctx.out.print(format!(
                "deleted {}, {} chunks removed{}",
                response.deleted,
                thousands(response.chunks_deleted as i64),
                if response.failed.is_empty() {
                    String::new()
                } else {
                    format!(", failed {}", response.failed.len())
                }
            ))?;
            for failure in &response.failed {
                ctx.out.warn(format!("{}: {}", failure.id, failure.code));
            }
            if response.index_cleanup_pending > 0 {
                ctx.out.warn(format!(
                    "{} document{} had their index cleanup queued for retry",
                    response.index_cleanup_pending,
                    if response.index_cleanup_pending == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
            }
        }
    }

    // 207 Multi-Status is the server saying "read the per-item outcomes".
    if !response.failed.is_empty() || status == 207 {
        return Err(CliError::PartialFailure(format!(
            "deleted {}, failed {} ({})",
            response.deleted,
            response.failed.len(),
            response
                .failed
                .iter()
                .take(3)
                .map(|f| format!("{}: {}", f.id, f.code))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> PageListArgs {
        PageListArgs::default()
    }

    #[test]
    fn stubs_asks_for_exactly_zero_chunks() {
        // chunk_min=0&chunk_max=0. Rows with a null count are excluded by the
        // server, which is the intended behaviour: "not counted" is not "empty".
        let mut a = args();
        a.stubs = true;
        assert_eq!(chunk_bounds(&a).unwrap(), (Some(0), Some(0)));
    }

    #[test]
    fn heavy_is_an_open_ended_lower_bound() {
        let mut a = args();
        a.heavy = true;
        assert_eq!(chunk_bounds(&a).unwrap(), (Some(11), None));
    }

    #[test]
    fn an_explicit_chunk_range_passes_through() {
        let mut a = args();
        a.chunks = Some("1..5".into());
        assert_eq!(chunk_bounds(&a).unwrap(), (Some(1), Some(5)));
    }

    #[test]
    fn no_chunk_filter_sends_no_bounds() {
        assert_eq!(chunk_bounds(&args()).unwrap(), (None, None));
    }

    #[test]
    fn a_bad_chunk_range_is_a_usage_error_before_any_request() {
        let mut a = args();
        a.chunks = Some("5..1".into());
        assert_eq!(
            chunk_bounds(&a).unwrap_err().exit_code(),
            crate::error::exit::USAGE
        );
    }

    #[test]
    fn ids_are_gathered_from_arguments_and_files_and_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ids.txt");
        std::fs::write(
            &path,
            "# a comment\nhttps://x/b\n\n  https://x/c  \nhttps://x/c\n",
        )
        .unwrap();

        let gathered =
            gather_ids(&["https://x/a".to_string()], Some(path.to_str().unwrap())).unwrap();
        assert_eq!(
            gathered,
            vec!["https://x/a", "https://x/b", "https://x/c"],
            "comments and blank lines are skipped, duplicates collapsed"
        );
    }

    #[test]
    fn duplicates_are_removed_even_when_they_are_not_adjacent() {
        // `Vec::dedup` would leave the second "a" in place, and the batch would
        // then report a spurious failure for it.
        let gathered = gather_ids(
            &[
                "https://x/a".to_string(),
                "https://x/b".to_string(),
                "https://x/a".to_string(),
            ],
            None,
        )
        .unwrap();
        assert_eq!(gathered, vec!["https://x/a", "https://x/b"]);
    }

    #[test]
    fn streamed_table_columns_have_fixed_widths_so_rows_line_up() {
        // Rows are rendered one at a time, so widths measured per row would pad
        // every line differently.
        let headers: Vec<String> = ["TITLE", "CONNECTOR", "CHUNKS", "UPDATED", "URL"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let widths = stream_column_widths(&headers);
        assert_eq!(widths.len(), headers.len());
        assert_eq!(widths[0], 60, "titles get real room");
        assert_eq!(widths[4], 0, "the last column is unbounded");
        assert!(widths.iter().take(4).all(|w| *w > 0));
    }

    #[test]
    fn a_missing_id_file_is_an_error_rather_than_an_empty_batch() {
        let err = gather_ids(&[], Some("/nonexistent/ids.txt")).unwrap_err();
        assert!(err.message().contains("cannot read"));
    }

    #[test]
    fn ids_that_look_like_urls_are_quoted_in_suggested_commands() {
        assert_eq!(shell_quote("https://x/y"), "https://x/y");
        assert_eq!(shell_quote("https://x/y?a=1"), "'https://x/y?a=1'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn the_reproduce_command_records_the_filters_that_produced_the_list() {
        let mut a = args();
        a.query = Some("kant".into());
        a.connector = Some("tildes".into());
        a.stubs = true;
        let described = describe_command(&a);
        assert!(described.contains("\"kant\""), "{described}");
        assert!(described.contains("-c tildes"), "{described}");
        assert!(described.contains("--stubs"), "{described}");
    }
}
