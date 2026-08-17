//! Overlays: help, confirm, picker, input.
//!
//! Every one draws on a cleared, centred rect and — because the event loop
//! routes keys to the overlay first — none of them lets a keystroke through to
//! the screen below.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{ConfirmState, InputState, PickerState, Screen};
use super::keys;
use super::theme;

/// A centred rect `percent_x` × `percent_y` of `area`.
pub fn centred(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn overlay_block(title: &str, danger: bool) -> Block<'_> {
    Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if danger {
            Style::default().fg(theme::ERROR)
        } else {
            Style::default().fg(theme::ACCENT)
        })
}

pub fn help(frame: &mut Frame, area: Rect, screen: Screen) {
    let rect = centred(72, 82, area);
    frame.render_widget(Clear, rect);

    let width = keys::for_screen(screen)
        .map(|b| b.keys.len())
        .max()
        .unwrap_or(12);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("{} screen", screen.title()),
            Style::default().fg(theme::MUTED),
        )),
        Line::from(""),
    ];
    for binding in keys::for_screen(screen) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:width$}  ", binding.keys, width = width),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(binding.help),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  press ? or Esc to close",
        Style::default().fg(theme::MUTED),
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .block(overlay_block("Keys", false))
            .wrap(Wrap { trim: false }),
        rect,
    );
}

pub fn confirm(frame: &mut Frame, area: Rect, state: &ConfirmState) {
    let height = (state.lines.len() as u16 + 6).min(area.height.saturating_sub(2));
    let rect = centred(70, 100, area);
    let rect = Rect {
        x: rect.x,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width: rect.width,
        height,
    };
    frame.render_widget(Clear, rect);

    let mut lines: Vec<Line> = state
        .lines
        .iter()
        .map(|line| Line::from(Span::raw(format!("  {line}"))))
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "y",
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" confirm    "),
        Span::styled(
            "n / Esc",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" cancel"),
    ]));

    frame.render_widget(
        Paragraph::new(lines)
            .block(overlay_block(&state.title, state.danger))
            .wrap(Wrap { trim: false }),
        rect,
    );
}

pub fn picker(frame: &mut Frame, area: Rect, state: &PickerState) {
    let rect = centred(60, 70, area);
    frame.render_widget(Clear, rect);

    let block = overlay_block(state.label, false);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ", Style::default().fg(theme::ACCENT)),
            Span::raw(state.query.as_str()),
            Span::styled("▏", Style::default().fg(theme::ACCENT)),
        ])),
        rows[0],
    );

    let visible = rows[1].height as usize;
    // Keep the cursor in view when the list is longer than the pane.
    let start = state.cursor.saturating_sub(visible.saturating_sub(1));
    let items: Vec<ListItem> = state
        .matches
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, label_index)| {
            let label = &state.labels[*label_index];
            if index == state.cursor {
                ListItem::new(format!("▸ {label}")).style(
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ListItem::new(format!("  {label}"))
            }
        })
        .collect();

    if items.is_empty() {
        frame.render_widget(
            Paragraph::new("  (no matches)").style(Style::default().fg(theme::MUTED)),
            rows[1],
        );
    } else {
        frame.render_widget(List::new(items), rows[1]);
    }
}

pub fn input(frame: &mut Frame, area: Rect, state: &InputState) {
    let rect = Rect {
        x: area.x + 2,
        y: area.y + area.height.saturating_sub(4),
        width: area.width.saturating_sub(4),
        height: 3,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(state.buffer.as_str()),
            // A rendered cursor: the old filter bar had none, so you could not
            // tell whether it was taking input.
            Span::styled("▏", Style::default().fg(theme::ACCENT)),
        ]))
        .block(overlay_block(state.label, false)),
        rect,
    );
}

/// Shown instead of everything else when the terminal is too small to lay out.
pub fn too_small(frame: &mut Frame, area: Rect, need: (u16, u16)) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "terminal too small",
                Style::default().fg(theme::WARN).bold(),
            )),
            Line::from(format!(
                "need at least {}×{}, have {}×{}",
                need.0, need.1, area.width, area.height
            )),
        ])
        .alignment(Alignment::Center),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_centred_rect_stays_inside_its_parent() {
        let area = Rect::new(0, 0, 100, 40);
        let rect = centred(60, 70, area);
        assert!(rect.x >= area.x && rect.y >= area.y);
        assert!(rect.right() <= area.right());
        assert!(rect.bottom() <= area.bottom());
        assert_eq!(rect.width, 60);
    }

    #[test]
    fn centring_a_tiny_area_does_not_panic_or_overflow() {
        for (w, h) in [(1, 1), (3, 2), (0, 0)] {
            let rect = centred(70, 70, Rect::new(0, 0, w, h));
            assert!(rect.right() <= w);
            assert!(rect.bottom() <= h);
        }
    }
}
