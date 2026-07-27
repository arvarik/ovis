use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::tui::app::{ActivePane, App};

/// Render the interactive Ratatui TUI dashboard.
pub fn draw(f: &mut Frame, app: &App) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Main Split Panes
            Constraint::Length(3), // Hotkeys / Footer
        ])
        .split(f.size());

    // --- 1. Header Bar ---
    let header_text = vec![
        Line::from(vec![
            Span::styled("OVIS v1.0 ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("| Onyx Visibility Terminal Dashboard ", Style::default().fg(Color::White)),
            Span::styled(
                format!("  [Total Docs: {} | Filtered: {}]", app.all_documents.len(), app.filtered_documents.len()),
                Style::default().fg(Color::Yellow),
            ),
        ]),
    ];
    let header = Paragraph::new(header_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" App Status "),
        )
        .alignment(Alignment::Left);
    f.render_widget(header, main_chunks[0]);

    // --- 2. Main Dual Split Pane Layout ---
    let split_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_chunks[1]);

    // --- Left Pane: Document List & Search & Pagination ---
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Search input
            Constraint::Min(5),    // Table list
            Constraint::Length(3), // Pagination bar
        ])
        .split(split_chunks[0]);

    // Search bar widget
    let search_border_style = if app.active_pane == ActivePane::SearchInput {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let search_text = if app.search_query.is_empty() && app.active_pane != ActivePane::SearchInput {
        Span::styled("Press '/' to search documents...", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(&app.search_query, Style::default().fg(Color::White))
    };

    let search_bar = Paragraph::new(Line::from(vec![Span::raw("🔍 "), search_text])).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(search_border_style)
            .title(" [1] Search Filter "),
    );
    f.render_widget(search_bar, left_chunks[0]);

    // Document Table widget
    let list_border_style = if app.active_pane == ActivePane::LeftList {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let selected_index = app.selected_index;
    let page_docs = app.current_page_documents();

    let rows: Vec<Row> = page_docs
        .iter()
        .enumerate()
        .map(|(idx, doc)| {
            let style = if idx == selected_index && app.active_pane != ActivePane::SearchInput {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let source = doc
                .metadata
                .get("source")
                .and_then(|s| s.as_str())
                .unwrap_or("web");

            let prefix = if idx == selected_index { "> " } else { "  " };
            let doc_title = format!("{}{}", prefix, doc.semantic_id);

            Row::new(vec![
                Span::raw(doc_title),
                Span::raw(source.to_string()),
            ])
            .style(style)
        })
        .collect();

    let widths = [Constraint::Percentage(70), Constraint::Percentage(30)];
    let doc_table = Table::new(rows, widths)
        .header(
            Row::new(vec!["TITLE / SEMANTIC ID", "SOURCE"])
                .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(list_border_style)
                .title(" DOCUMENT LIST "),
        );

    f.render_widget(doc_table, left_chunks[1]);

    // Pagination Widget
    let pagination_text = format!(
        "Page {} of {}  |  Showing {} items",
        app.page,
        app.total_pages(),
        page_docs.len()
    );
    let pagination_bar = Paragraph::new(pagination_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Pagination [n/p] "),
        )
        .alignment(Alignment::Center);
    f.render_widget(pagination_bar, left_chunks[2]);

    // --- Right Pane: Notion-Style Document Chunk/Metadata Inspector Drawer ---
    let right_border_style = if app.active_pane == ActivePane::RightInspector {
        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut inspector_lines = Vec::new();

    if let Some(doc) = app.selected_document() {
        inspector_lines.push(Line::from(vec![
            Span::styled("Document ID: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(&doc.id, Style::default().fg(Color::White)),
        ]));
        inspector_lines.push(Line::from(vec![
            Span::styled("Title:       ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(&doc.semantic_id, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]));
        if let Some(link) = &doc.link {
            inspector_lines.push(Line::from(vec![
                Span::styled("Link:        ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(link, Style::default().fg(Color::Cyan)),
            ]));
        }
        if let Some(updated) = &doc.doc_updated_at {
            inspector_lines.push(Line::from(vec![
                Span::styled("Updated At:  ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(updated.format("%Y-%m-%d %H:%M:%S UTC").to_string(), Style::default().fg(Color::White)),
            ]));
        }
        if let Some(owners) = &doc.primary_owners {
            inspector_lines.push(Line::from(vec![
                Span::styled("Owners:      ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(owners.join(", "), Style::default().fg(Color::White)),
            ]));
        }

        inspector_lines.push(Line::from("─".repeat(50)));
        inspector_lines.push(Line::from(vec![
            Span::styled("Metadata JSON: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]));

        let meta_json = serde_json::to_string_pretty(&doc.metadata).unwrap_or_default();
        for line in meta_json.lines() {
            inspector_lines.push(Line::from(Span::styled(format!("  {}", line), Style::default().fg(Color::Gray))));
        }

        inspector_lines.push(Line::from("─".repeat(50)));

        let chunks = app.selected_chunks();
        inspector_lines.push(Line::from(vec![
            Span::styled(
                format!("Vector Chunks Breakdown ({})", chunks.len()),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]));

        if chunks.is_empty() {
            inspector_lines.push(Line::from(Span::styled(
                "  [No raw vector chunks indexed for this page]",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for chunk in chunks {
                inspector_lines.push(Line::from(vec![
                    Span::styled(
                        format!("• Chunk #{} (ID: {})", chunk.chunk_id, chunk.document_id),
                        Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
                    ),
                ]));
                for line in chunk.content.lines().take(6) {
                    inspector_lines.push(Line::from(Span::styled(
                        format!("    {}", line),
                        Style::default().fg(Color::White),
                    )));
                }
            }
        }
    } else {
        inspector_lines.push(Line::from(Span::styled(
            "No document selected.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let inspector = Paragraph::new(inspector_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(right_border_style)
                .title(" PAGE INSPECTOR DRAWER "),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(inspector, split_chunks[1]);

    // --- 3. Bottom Hotkeys & Status Bar ---
    let status_text = app.status_message.as_deref().unwrap_or("Ready");
    let hotkeys_text = vec![
        Line::from(vec![
            Span::styled(" [Tab] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("Switch Pane | "),
            Span::styled(" [/] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("Search | "),
            Span::styled(" [d] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("Delete Page | "),
            Span::styled(" [n/p] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("Page +/- | "),
            Span::styled(" [Esc] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("Clear Filter | "),
            Span::styled(" [q] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("Quit"),
        ]),
        Line::from(vec![
            Span::styled(" Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(status_text, Style::default().fg(Color::White)),
        ]),
    ];

    let footer = Paragraph::new(hotkeys_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Hotkeys & Controls "),
    );
    f.render_widget(footer, main_chunks[2]);

    // --- 4. Delete Confirmation Modal Overlay ---
    if let Some(doc_id) = &app.delete_confirm {
        let area = centered_rect(60, 20, f.size());
        f.render_widget(Clear, area);

        let modal_text = vec![
            Line::from(Span::styled("Confirm Document Deletion", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(vec![
                Span::raw("Are you sure you want to delete "),
                Span::styled(doc_id, Style::default().fg(Color::Yellow)),
                Span::raw("?"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(" [y] Yes, Delete ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw("    "),
                Span::styled(" [n / Esc] Cancel ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            ]),
        ];

        let popup = Paragraph::new(modal_text)
            .block(
                Block::default()
                    .title(" CONFIRMATION ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(Color::Red)),
            )
            .alignment(Alignment::Center);

        f.render_widget(popup, area);
    }
}

/// Helper function to calculate centered Rect for modal overlays.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
