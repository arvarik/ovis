//! The three screens.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    Wrap,
};
use ratatui::Frame;

use ovis_core::api_types::*;

use super::app::{App, Focus, InspectorTab, PagesSource, SEARCH_MODES, SORTS};
use super::theme;
use crate::output::{bytes, relative_time, thousands};

// Column thresholds are measured against the *pane*, not the terminal: the list
// occupies 45–55% of the width, so a 150-column terminal gives it about 67.
// Columns drop in order of how much they earn their space — the title and chunk
// count always survive.
/// Enough room for a connector column beside the title.
const FITS_CONNECTOR: u16 = 52;
/// Enough room for a relative timestamp as well.
const FITS_UPDATED: u16 = 72;
/// Below this the inspector is dropped and the list gets the whole width.
const VERY_NARROW: u16 = 70;

fn pane(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default().fg(theme::ACCENT)
        } else {
            Style::default().fg(theme::MUTED)
        })
}

fn ago(ts: &chrono::DateTime<chrono::Utc>) -> String {
    relative_time(ts, chrono::Utc::now())
}

fn ago_opt(ts: Option<&chrono::DateTime<chrono::Utc>>) -> String {
    ts.map(ago).unwrap_or_else(|| "—".into())
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

pub fn pages(frame: &mut Frame, app: &mut App, area: Rect) {
    // A narrow terminal gets the list alone; splitting it would leave neither
    // pane usable.
    let split = if area.width < VERY_NARROW {
        vec![area]
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area)
            .to_vec()
    };

    pages_list(frame, app, split[0]);
    if split.len() > 1 {
        pages_inspector(frame, app, split[1]);
    }
}

fn pages_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let state = &app.pages;
    let searching = state.source == PagesSource::Search;

    let mut title = if searching {
        format!("search: {}", state.filter)
    } else if state.filter.is_empty() {
        "pages".to_string()
    } else {
        // The active filter is always visible — the old TUI's pre-filter was
        // invisible, so a launch flag looked like an empty database.
        format!("filter: {:?}", state.filter)
    };
    if let Some((_, _, name)) = &state.connector {
        title.push_str(&format!(" · {name}"));
    }
    if !state.marks.is_empty() {
        title.push_str(&format!(" · {} marked", state.marks.len()));
    }

    let show_connector = area.width >= FITS_CONNECTOR;
    let show_updated = area.width >= FITS_UPDATED;
    let mut header = vec!["TITLE"];
    let mut widths = vec![Constraint::Min(20)];
    if searching {
        header.insert(0, "SCORE");
        widths.insert(0, Constraint::Length(6));
    }
    if show_connector {
        header.push("CONNECTOR");
        widths.push(Constraint::Length(16));
    }
    header.push("CH");
    widths.push(Constraint::Length(4));
    if show_updated {
        header.push("UPDATED");
        widths.push(Constraint::Length(9));
    }

    let rows: Vec<Row> = state
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let marked = state.marks.contains(&item.id);
            let mut cells: Vec<Cell> = Vec::new();
            if searching {
                let score = state
                    .hits
                    .get(index)
                    .map(|h| format!("{:.1}", h.score))
                    .unwrap_or_default();
                cells.push(Cell::from(score).style(Style::default().fg(theme::MUTED)));
            }
            cells.push(Cell::from(format!(
                "{}{}",
                if marked { "✓ " } else { "" },
                item.semantic_id
            )));
            if show_connector {
                cells.push(Cell::from(
                    item.connector_name.clone().unwrap_or_else(|| "—".into()),
                ));
            }
            cells.push(match item.chunk_count {
                // null is "not counted yet", which is not zero.
                None => Cell::from("—").style(Style::default().fg(theme::MUTED)),
                Some(n) => Cell::from(n.to_string()),
            });
            if show_updated {
                cells.push(Cell::from(ago(&item.updated_at)));
            }
            let row = Row::new(cells);
            if marked {
                row.style(Style::default().fg(theme::ACCENT))
            } else {
                row
            }
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(
            Row::new(header).style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(pane(title, app.focus == Focus::List))
        .row_highlight_style(
            Style::default()
                .bg(theme::ACCENT)
                .fg(ratatui::style::Color::Black)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(table, area, &mut app.pages.table);
}

fn pages_inspector(frame: &mut Frame, app: &mut App, area: Rect) {
    let state = &app.pages;
    let focused = app.focus == Focus::Inspector;

    let tabs: String = [
        InspectorTab::Overview,
        InspectorTab::Text,
        InspectorTab::Chunks,
        InspectorTab::Json,
    ]
    .iter()
    .map(|t| {
        if *t == state.tab {
            format!("[{}]", t.title())
        } else {
            format!(" {} ", t.title())
        }
    })
    .collect::<Vec<_>>()
    .join("");

    let block = pane(tabs, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(item) = state.selected() else {
        frame.render_widget(
            Paragraph::new("no selection").style(Style::default().fg(theme::MUTED)),
            inner,
        );
        return;
    };

    let lines: Vec<Line> = match state.tab {
        InspectorTab::Overview => overview_lines(state, item),
        InspectorTab::Text => match &state.text {
            Some(text) => text.lines().map(|l| Line::from(l.to_string())).collect(),
            None => vec![Line::from(Span::styled(
                "loading text…",
                Style::default().fg(theme::MUTED),
            ))],
        },
        InspectorTab::Chunks => match &state.chunks {
            Some(chunks) => chunk_lines(chunks),
            None => vec![Line::from(Span::styled(
                "loading chunks…",
                Style::default().fg(theme::MUTED),
            ))],
        },
        InspectorTab::Json => match &state.detail {
            Some(detail) => serde_json::to_string_pretty(detail)
                .unwrap_or_default()
                .lines()
                .map(|l| Line::from(l.to_string()))
                .collect(),
            None => vec![Line::from("loading…")],
        },
    };

    let total = lines.len();
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            // The old inspector could not scroll at all; content past the pane
            // was silently dropped.
            .scroll((state.inspector_scroll, 0)),
        inner,
    );

    if total > inner.height as usize {
        let mut scrollbar = ScrollbarState::new(total.saturating_sub(inner.height as usize))
            .position(state.inspector_scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            inner,
            &mut scrollbar,
        );
    }
}

fn overview_lines<'a>(state: &'a super::app::PagesState, item: &'a PageListItem) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(Span::styled(
            item.semantic_id.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            item.link.clone().unwrap_or_else(|| item.id.clone()),
            Style::default().fg(theme::ACCENT),
        )),
        Line::from(""),
    ];

    let detail = state.detail.as_ref().filter(|d| d.item.id == item.id);

    lines.push(Line::from(vec![
        Span::styled("connector  ", Style::default().fg(theme::MUTED)),
        Span::raw(item.connector_name.clone().unwrap_or_else(|| "—".into())),
        Span::raw(" "),
        Span::styled(
            detail
                .and_then(|d| d.cc_pair_status.clone())
                .unwrap_or_default(),
            Style::default().fg(theme::status(
                detail
                    .and_then(|d| d.cc_pair_status.as_deref())
                    .unwrap_or(""),
            )),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("chunks     ", Style::default().fg(theme::MUTED)),
        Span::raw(
            item.chunk_count
                .map(|c| c.to_string())
                .unwrap_or_else(|| "not counted yet".into()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("updated    ", Style::default().fg(theme::MUTED)),
        Span::raw(ago(&item.updated_at)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("boost      ", Style::default().fg(theme::MUTED)),
        Span::raw(item.boost.to_string()),
        Span::raw(if item.hidden { "  · hidden" } else { "" }),
    ]));

    if let Some(detail) = detail {
        if !detail.tags.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("tags       ", Style::default().fg(theme::MUTED)),
                Span::raw(
                    detail
                        .tags
                        .iter()
                        .map(|t| format!("{}={}", t.key, t.value))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ]));
        }
        // Both of these are the API being honest about something awkward, so
        // they are prominent rather than tucked into the JSON tab.
        if detail.recrawl_risk {
            lines.push(Line::from(Span::styled(
                "⚠ recrawl risk: the connector is active, so a delete is likely to be undone",
                Style::default().fg(theme::WARN),
            )));
        }
        if !detail.pg_row {
            lines.push(Line::from(Span::styled(
                "⚠ no Postgres row: the index holds orphaned chunks for this id",
                Style::default().fg(theme::ERROR),
            )));
        }
    }

    if let Some(text) = &state.text {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "── text preview ─────────────",
            Style::default().fg(theme::MUTED),
        )));
        for line in text.lines().take(40) {
            lines.push(Line::from(line.to_string()));
        }
    }
    lines
}

fn chunk_lines(chunks: &ChunksResponse) -> Vec<Line<'_>> {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{} of {} chunks · {} ({}d)",
            chunks.items.len(),
            thousands(chunks.total_chunks),
            chunks.embedding_model,
            chunks.embedding_dim
        ),
        Style::default().fg(theme::MUTED),
    ))];
    for chunk in &chunks.items {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "#{}  {} words",
                chunk.chunk_index,
                chunk.token_estimate.unwrap_or(0)
            ),
            Style::default().fg(theme::ACCENT),
        )));
        let body = chunk
            .content
            .as_deref()
            .or(chunk.blurb.as_deref())
            .unwrap_or("");
        for line in body.lines() {
            lines.push(Line::from(line.to_string()));
        }
    }
    lines
}

// ---------------------------------------------------------------------------
// Connectors
// ---------------------------------------------------------------------------

pub fn connectors(frame: &mut Frame, app: &mut App, area: Rect) {
    let split = if area.width < VERY_NARROW {
        vec![area]
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area)
            .to_vec()
    };

    let show_source = split[0].width >= FITS_CONNECTOR;
    let show_attempt = split[0].width >= FITS_UPDATED;
    let mut header = vec!["NAME", "STATUS", "DOCS"];
    let mut widths = vec![
        Constraint::Min(14),
        Constraint::Length(18),
        Constraint::Length(9),
    ];
    if show_source {
        header.push("SOURCE");
        widths.push(Constraint::Length(8));
    }
    if show_attempt {
        header.push("LAST ATTEMPT");
        widths.push(Constraint::Length(18));
    }

    let rows: Vec<Row> = app
        .connectors
        .items
        .iter()
        .map(|c| {
            let mut status = c.status.clone();
            let mut colour = theme::status(&c.status);
            if c.parked {
                status.push_str(" ⏸");
                colour = theme::WARN;
            }
            if c.in_repeated_error_state {
                status.push_str(" ⚠");
                colour = theme::ERROR;
            }
            let mut cells = vec![
                Cell::from(c.name.clone()),
                Cell::from(status).style(Style::default().fg(colour)),
                Cell::from(thousands(c.doc_count)),
            ];
            if show_source {
                cells.push(Cell::from(c.source.clone()));
            }
            if show_attempt {
                cells.push(match &c.last_attempt {
                    Some(a) => {
                        let label = a.status.clone().unwrap_or_else(|| "—".into());
                        Cell::from(format!("{label} {}", ago_opt(a.time_updated.as_ref())))
                            .style(Style::default().fg(theme::status(&label)))
                    }
                    None => Cell::from("—").style(Style::default().fg(theme::MUTED)),
                });
            }
            Row::new(cells)
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(
            Row::new(header).style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(pane(
            format!("connectors ({})", app.connectors.items.len()),
            app.focus == Focus::List,
        ))
        .row_highlight_style(
            Style::default()
                .bg(theme::ACCENT)
                .fg(ratatui::style::Color::Black)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(table, split[0], &mut app.connectors.table);

    if split.len() > 1 {
        connector_detail(frame, app, split[1]);
    }
}

fn connector_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane("detail".into(), app.focus == Focus::Inspector);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // An error drill-in replaces the detail pane for the connector it belongs to.
    if let Some((cc_pair_id, errors, window)) = &app.connectors.errors {
        if app.connectors.selected().map(|c| c.cc_pair_id) == Some(*cc_pair_id) {
            let mut lines = vec![
                Line::from(Span::styled(
                    format!("{} indexing errors · window {window}", errors.len()),
                    Style::default().fg(theme::MUTED),
                )),
                Line::from(""),
            ];
            for error in errors {
                lines.push(Line::from(Span::styled(
                    error
                        .document_link
                        .clone()
                        .or_else(|| error.document_id.clone())
                        .unwrap_or_else(|| "(no document)".into()),
                    Style::default().fg(theme::ACCENT),
                )));
                lines.push(Line::from(Span::styled(
                    format!("  {}", error.failure_message.trim()),
                    Style::default().fg(theme::ERROR),
                )));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((app.connectors.inspector_scroll, 0)),
                inner,
            );
            return;
        }
    }

    let Some(summary) = app.connectors.selected() else {
        frame.render_widget(
            Paragraph::new("no selection").style(Style::default().fg(theme::MUTED)),
            inner,
        );
        return;
    };

    let detail = app
        .connectors
        .detail
        .as_ref()
        .filter(|d| d.summary.cc_pair_id == summary.cc_pair_id);

    let mut lines = vec![
        Line::from(Span::styled(
            summary.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "cc-pair {} · connector {} · {}",
                summary.cc_pair_id, summary.connector_id, summary.source
            ),
            Style::default().fg(theme::MUTED),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("status     ", Style::default().fg(theme::MUTED)),
            Span::styled(
                summary.status.clone(),
                Style::default().fg(theme::status(&summary.status)),
            ),
        ]),
        Line::from(vec![
            Span::styled("documents  ", Style::default().fg(theme::MUTED)),
            Span::raw(thousands(summary.doc_count)),
        ]),
        Line::from(vec![
            Span::styled("last ok    ", Style::default().fg(theme::MUTED)),
            Span::raw(ago_opt(summary.last_successful_index_time.as_ref())),
        ]),
    ];

    if summary.parked {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "⏸ parked by the resilience cron — a first-pass crawl is already complete.",
            Style::default().fg(theme::WARN),
        )));
        lines.push(Line::from(Span::styled(
            "  O will ask before overriding that.",
            Style::default().fg(theme::WARN),
        )));
    }

    if let Some(detail) = detail {
        let a = &detail.attempts;
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("attempts   ", Style::default().fg(theme::MUTED)),
            Span::styled(format!("{} ok", a.success), Style::default().fg(theme::OK)),
            Span::raw(" · "),
            Span::styled(
                format!("{} failed", a.failed),
                Style::default().fg(theme::ERROR),
            ),
            Span::raw(format!(" · {} canceled", a.canceled)),
        ]));

        if let Some(history) = &detail.history {
            let max = history.iter().map(|p| p.docs_added).max().unwrap_or(0);
            let sparkline: String = history.iter().map(|p| spark(p.docs_added, max)).collect();
            lines.push(Line::from(vec![
                Span::styled("14 days    ", Style::default().fg(theme::MUTED)),
                Span::styled(sparkline, Style::default().fg(theme::ACCENT)),
                Span::raw(format!(
                    "  {} docs",
                    thousands(history.iter().map(|p| p.docs_added).sum())
                )),
            ]));
        }

        if let Some(config) = &detail.connector_specific_config {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "config",
                Style::default().fg(theme::MUTED),
            )));
            for line in serde_json::to_string_pretty(config)
                .unwrap_or_default()
                .lines()
            {
                lines.push(Line::from(line.to_string()));
            }
        }
    }

    if let Some(last) = &summary.last_attempt {
        if let Some(msg) = &last.error_msg {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("last error: {}", msg.trim()),
                Style::default().fg(theme::MUTED),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.connectors.inspector_scroll, 0)),
        inner,
    );
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
// Activity
// ---------------------------------------------------------------------------

pub fn activity(frame: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(3)])
        .split(area);

    activity_header(frame, app, rows[0]);

    let show_rate = area.width >= 90;
    let mut header = vec!["ID", "CONNECTOR", "STATUS", "DOCS"];
    let mut widths = vec![
        Constraint::Length(7),
        Constraint::Min(14),
        Constraint::Length(20),
        Constraint::Length(16),
    ];
    if show_rate {
        header.push("RATE");
        widths.push(Constraint::Length(10));
        header.push("UPDATED");
        widths.push(Constraint::Length(10));
    }

    let table_rows: Vec<Row> = app
        .activity
        .attempts
        .iter()
        .map(|a| {
            let mut status = a.status.clone();
            let mut colour = theme::status(&a.status);
            if a.stalled {
                status.push_str(" ⚠STALLED");
                colour = theme::ERROR;
            }
            if a.parked {
                status.push_str(" ⏸");
                colour = theme::WARN;
            }
            let mut cells = vec![
                Cell::from(a.id.to_string()),
                Cell::from(a.connector_name.clone().unwrap_or_else(|| "—".into())),
                Cell::from(status).style(Style::default().fg(colour)),
                Cell::from(format!(
                    "{} / {}",
                    a.new_docs_indexed.unwrap_or(0),
                    a.total_docs_indexed.unwrap_or(0)
                )),
            ];
            if show_rate {
                cells.push(match a.pages_per_min {
                    Some(rate) => Cell::from(format!("{rate:.1}/min")),
                    None => Cell::from("—").style(Style::default().fg(theme::MUTED)),
                });
                cells.push(Cell::from(ago(&a.time_updated)));
            }
            Row::new(cells)
        })
        .collect();

    let title = match &app.activity.scope {
        Some(name) => format!("attempts · {name}"),
        None => format!(
            "attempts{}",
            if app.activity.frozen {
                " · frozen"
            } else {
                ""
            }
        ),
    };

    let table = Table::new(table_rows, widths)
        .header(
            Row::new(header).style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(pane(title, true))
        .row_highlight_style(
            Style::default()
                .bg(theme::ACCENT)
                .fg(ratatui::style::Color::Black)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(table, rows[1], &mut app.activity.table);
}

fn activity_header(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane("crawl".into(), false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(stats) = &app.activity.stats else {
        frame.render_widget(
            Paragraph::new("loading…").style(Style::default().fg(theme::MUTED)),
            inner,
        );
        return;
    };

    let crawl = &stats.crawl;
    let index = &stats.index;
    let disk_pct = index.disk_used_pct.unwrap_or(0.0);
    // The 400 GB index has tripped the flood-stage watermark before, so disk is
    // a permanent fixture of this header rather than something you go looking for.
    let disk_colour = if index.read_only || disk_pct >= 90.0 {
        theme::ERROR
    } else if disk_pct >= 80.0 {
        theme::WARN
    } else {
        theme::OK
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("docs  ", Style::default().fg(theme::MUTED)),
            Span::raw(format!(
                "{} last 15m · {} last 24h",
                thousands(crawl.docs_last_15m),
                thousands(crawl.docs_last_24h)
            )),
            Span::raw("   "),
            Span::styled("running  ", Style::default().fg(theme::MUTED)),
            Span::raw(crawl.attempts_in_progress.to_string()),
            if crawl.attempts_stalled > 0 {
                Span::styled(
                    format!("   {} STALLED", crawl.attempts_stalled),
                    Style::default()
                        .fg(theme::ERROR)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            },
        ]),
        Line::from(vec![
            Span::styled("index ", Style::default().fg(theme::MUTED)),
            Span::styled(
                format!(
                    "{} · {:.0}% disk used · {} free",
                    index.size_bytes.map(bytes).unwrap_or_else(|| "—".into()),
                    disk_pct,
                    index
                        .disk_available_bytes
                        .map(bytes)
                        .unwrap_or_else(|| "—".into())
                ),
                Style::default().fg(disk_colour),
            ),
            if index.read_only {
                Span::styled(
                    "   ⚠ READ-ONLY (flood stage)",
                    Style::default()
                        .fg(theme::ERROR)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            },
        ]),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Chrome
// ---------------------------------------------------------------------------

pub fn header(frame: &mut Frame, app: &App, area: Rect) {
    let docs = app
        .activity
        .stats
        .as_ref()
        .map(|s| {
            format!(
                "{}{} docs",
                if s.documents_exact { "" } else { "~" },
                thousands(s.documents)
            )
        })
        .unwrap_or_else(|| "…".into());

    let mut spans = vec![
        Span::styled(
            " OVIS ",
            Style::default().fg(theme::OK).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("─ {} ─ ", app.server),
            Style::default().fg(theme::MUTED),
        ),
        Span::raw(docs),
        Span::raw("   "),
    ];
    for (index, screen) in [
        super::app::Screen::Pages,
        super::app::Screen::Connectors,
        super::app::Screen::Activity,
    ]
    .iter()
    .enumerate()
    {
        let selected = *screen == app.screen;
        spans.push(Span::styled(
            format!("[{}]{} ", index + 1, screen.title()),
            if selected {
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::MUTED)
            },
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn status_bar(frame: &mut Frame, app: &App, area: Rect) {
    // A toast wins the line: an action's outcome matters more than the summary.
    if let Some(toast) = &app.toast {
        if !toast.expired() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {}", toast.text),
                    Style::default()
                        .fg(if toast.error { theme::ERROR } else { theme::OK })
                        .add_modifier(Modifier::BOLD),
                ))),
                area,
            );
            return;
        }
    }

    let text = match app.screen {
        super::app::Screen::Pages => {
            let state = &app.pages;
            let position = match state.table.selected() {
                Some(index) => format!("{}/{}", index + 1, state.items.len()),
                None => format!("0/{}", state.items.len()),
            };
            let mut line = format!(
                " {position} of {}{} · sorted {}",
                if state.total_exact { "" } else { "~" },
                thousands(state.total),
                SORTS[state.sort_index].1
            );
            if state.source == PagesSource::Search {
                line.push_str(&format!(
                    " · {} {}ms",
                    SEARCH_MODES[state.search_mode], state.took_ms
                ));
                // Without this the two vector modes look silently broken: they
                // run, they return keyword results, and nothing says so.
                if let Some(degraded) = &state.degraded {
                    line.push_str(&format!(" · degraded: {degraded}"));
                }
            }
            if state.loading {
                line.push_str(" · loading…");
            }
            line.push_str("  ⏎ inspect  x mark  d delete  ? help  q quit");
            line
        }
        super::app::Screen::Connectors => {
            " P pause  R resume  O run once  e errors  a attempts  ⏎ pages  ? help  q quit"
                .to_string()
        }
        super::app::Screen::Activity => format!(
            " {} attempts · auto-refresh {}  f freeze  r refresh  ? help  q quit",
            app.activity.attempts.len(),
            if app.activity.frozen {
                "off".to_string()
            } else {
                format!("{}s", app.auto_refresh.as_secs())
            }
        ),
    };

    let style = if app.toast.as_ref().is_some_and(|t| t.error && !t.expired()) {
        Style::default().fg(theme::ERROR)
    } else {
        Style::default().fg(theme::MUTED)
    };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_buckets_never_index_out_of_range() {
        assert_eq!(spark(0, 0), '▁');
        assert_eq!(spark(100, 100), '█');
        assert_eq!(spark(200, 100), '█', "a value above the max clamps");
        assert_eq!(spark(-5, 100), '▁');
    }

    #[test]
    fn relative_stamps_render_for_present_and_absent_timestamps() {
        assert_eq!(ago_opt(None), "—");
        let now = chrono::Utc::now();
        assert_eq!(ago_opt(Some(&now)), "just now");
    }

    #[test]
    fn the_narrow_thresholds_leave_room_for_the_columns_they_gate() {
        // Thresholds are pane-relative: a 150-column terminal gives the list
        // about 67, which must be enough for the connector column. Below
        // VERY_NARROW the inspector is dropped entirely, and that threshold has
        // to sit at or above the 60-column min-size guard.
        const { assert!(FITS_CONNECTOR < 67) };
        const { assert!(FITS_UPDATED > FITS_CONNECTOR) };
        const { assert!(VERY_NARROW >= 60) };
    }
}
