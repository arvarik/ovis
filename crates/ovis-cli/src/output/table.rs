//! Tabular data: column selection, rendering, CSV.
//!
//! One shape (`Grid`) feeds the boxed table, the plain aligned table, and the
//! CSV writer, so `--columns` means the same thing in all three and the columns
//! cannot drift between formats the way the hand-rolled CSV writer used to.

use comfy_table::{Cell, ContentArrangement, Table};

use super::style::Tone;

/// One column a command can render.
#[derive(Debug, Clone, Copy)]
pub struct ColSpec {
    pub name: &'static str,
    /// Shown only under `--wide` (or an explicit `--columns`).
    pub wide_only: bool,
}

impl ColSpec {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            wide_only: false,
        }
    }
    pub const fn wide(name: &'static str) -> Self {
        Self {
            name,
            wide_only: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GridCell {
    pub text: String,
    pub tone: Tone,
}

impl GridCell {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Plain,
        }
    }
    pub fn toned(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Grid {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<GridCell>>,
}

impl Grid {
    pub fn new(headers: Vec<String>) -> Self {
        Self {
            headers,
            rows: Vec::new(),
        }
    }

    pub fn push(&mut self, row: Vec<GridCell>) {
        debug_assert_eq!(
            row.len(),
            self.headers.len(),
            "row width must match the header width"
        );
        self.rows.push(row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Resolve which columns to render, in which order.
///
/// `--columns` wins outright (including its ordering); otherwise `--wide` adds
/// the wide-only set. An unknown name is an error listing what exists, rather
/// than a silently missing column.
pub fn select_columns(
    specs: &[ColSpec],
    wide: bool,
    requested: Option<&str>,
) -> Result<Vec<&'static str>, String> {
    match requested {
        Some(list) => {
            let mut chosen = Vec::new();
            for raw in list.split(',') {
                let name = raw.trim();
                if name.is_empty() {
                    continue;
                }
                match specs.iter().find(|s| s.name.eq_ignore_ascii_case(name)) {
                    Some(spec) => chosen.push(spec.name),
                    None => {
                        let known: Vec<&str> = specs.iter().map(|s| s.name).collect();
                        return Err(format!(
                            "unknown column '{name}'; available: {}",
                            known.join(", ")
                        ));
                    }
                }
            }
            if chosen.is_empty() {
                return Err("--columns was given but selected nothing".into());
            }
            Ok(chosen)
        }
        None => Ok(specs
            .iter()
            .filter(|s| wide || !s.wide_only)
            .map(|s| s.name)
            .collect()),
    }
}

/// The boxed, coloured table for a terminal.
pub fn render_boxed(grid: &Grid, color: bool, max_width: u16) -> String {
    let mut table = Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL);
    // Dynamic arrangement everywhere: the old `page inspect` table overflowed
    // any terminal narrower than its widest cell.
    table.set_content_arrangement(ContentArrangement::Dynamic);
    if max_width > 0 {
        table.set_width(max_width);
    }
    if !color {
        table.force_no_tty();
    }

    table.set_header(
        grid.headers
            .iter()
            .map(|h| {
                let cell = Cell::new(h);
                if color {
                    cell.add_attribute(comfy_table::Attribute::Bold)
                } else {
                    cell
                }
            })
            .collect::<Vec<_>>(),
    );

    for row in &grid.rows {
        table.add_row(
            row.iter()
                .map(|c| {
                    let cell = Cell::new(&c.text);
                    match (color, c.tone.comfy()) {
                        (true, Some(colour)) => cell.fg(colour),
                        _ => cell,
                    }
                })
                .collect::<Vec<_>>(),
        );
    }

    table.to_string()
}

/// Plain aligned columns — what a pipe gets. No box art, no colour, no
/// truncation: whatever consumes this deserves the whole value.
pub fn render_plain(grid: &Grid, headers: bool) -> String {
    let widths = natural_widths(grid, headers);
    render_plain_with(grid, headers, &widths)
}

/// Widths that fit `grid`'s own contents.
pub fn natural_widths(grid: &Grid, headers: bool) -> Vec<usize> {
    let mut widths: Vec<usize> = if headers {
        grid.headers.iter().map(|h| display_width(h)).collect()
    } else {
        vec![0; grid.headers.len()]
    };
    for row in &grid.rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(display_width(&cell.text));
            }
        }
    }
    widths
}

/// Render against widths chosen by the caller.
///
/// Streaming (`--all`) needs this: each row is rendered on its own as it
/// arrives, so widths derived per row would pad every line differently and
/// nothing would line up. Fixed widths keep a streamed table readable without
/// buffering the result set to measure it.
pub fn render_plain_with(grid: &Grid, headers: bool, widths: &[usize]) -> String {
    let mut out = String::new();
    if headers {
        push_row(&mut out, grid.headers.iter().map(String::as_str), widths);
    }
    for row in &grid.rows {
        push_row(&mut out, row.iter().map(|c| c.text.as_str()), widths);
    }
    out
}

fn push_row<'a>(out: &mut String, cells: impl Iterator<Item = &'a str>, widths: &[usize]) {
    let cells: Vec<&str> = cells.collect();
    let last = cells.len().saturating_sub(1);
    for (i, text) in cells.iter().enumerate() {
        let mut flat = text.replace(['\n', '\t'], " ");
        let width = widths.get(i).copied().unwrap_or(0);
        if i == last {
            // The last column is never truncated: it is where the long values
            // live (URLs), and nothing lines up after it anyway.
            out.push_str(flat.trim_end());
        } else {
            // An over-long cell would otherwise shove every column after it out
            // of line. With natural widths this never fires, because the widths
            // were measured from these very cells.
            if width > 0 && display_width(&flat) > width {
                flat = truncate(&flat, width);
            }
            out.push_str(&flat);
            let pad = width.saturating_sub(display_width(&flat));
            out.push_str(&" ".repeat(pad + 2));
        }
    }
    out.push('\n');
}

/// CSV through the `csv` crate — the hand-rolled writer escaped quotes wrongly
/// and drifted from the table's columns.
pub fn render_csv(grid: &Grid, headers: bool) -> Result<String, csv::Error> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    if headers {
        writer.write_record(&grid.headers)?;
    }
    for row in &grid.rows {
        writer.write_record(row.iter().map(|c| c.text.as_str()))?;
    }
    let bytes = writer.into_inner().map_err(|e| e.into_error())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Truncate to `max` display columns, with an ellipsis when it had to cut.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 || display_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut width = 0;
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        width += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: &[ColSpec] = &[
        ColSpec::new("title"),
        ColSpec::new("chunks"),
        ColSpec::wide("id"),
        ColSpec::wide("boost"),
    ];

    fn grid() -> Grid {
        let mut g = Grid::new(vec!["A".into(), "B".into()]);
        g.push(vec![GridCell::plain("one"), GridCell::plain("2")]);
        g.push(vec![
            GridCell::plain("a \"quoted\", comma"),
            GridCell::toned("3", Tone::Ok),
        ]);
        g
    }

    #[test]
    fn wide_only_columns_are_hidden_by_default_and_shown_with_wide() {
        assert_eq!(
            select_columns(SPECS, false, None).unwrap(),
            vec!["title", "chunks"]
        );
        assert_eq!(
            select_columns(SPECS, true, None).unwrap(),
            vec!["title", "chunks", "id", "boost"]
        );
    }

    #[test]
    fn explicit_columns_control_order_and_may_include_wide_ones() {
        assert_eq!(
            select_columns(SPECS, false, Some("id,title")).unwrap(),
            vec!["id", "title"]
        );
        assert_eq!(
            select_columns(SPECS, false, Some(" TITLE , chunks ")).unwrap(),
            vec!["title", "chunks"]
        );
    }

    #[test]
    fn an_unknown_column_names_the_available_ones_rather_than_vanishing() {
        let err = select_columns(SPECS, false, Some("titel")).unwrap_err();
        assert!(err.contains("titel"), "{err}");
        assert!(err.contains("title"), "{err}");
    }

    #[test]
    fn csv_quoting_is_correct_where_the_hand_rolled_writer_was_not() {
        let csv = render_csv(&grid(), true).unwrap();
        let mut lines = csv.lines();
        assert_eq!(lines.next().unwrap(), "A,B");
        assert_eq!(lines.next().unwrap(), "one,2");
        // A quote inside a quoted field doubles; the old writer emitted a raw ".
        assert_eq!(lines.next().unwrap(), r#""a ""quoted"", comma",3"#);
    }

    #[test]
    fn csv_can_omit_headers_for_pipelines() {
        let csv = render_csv(&grid(), false).unwrap();
        assert!(!csv.starts_with("A,B"));
        assert!(csv.starts_with("one,2"));
    }

    #[test]
    fn plain_rendering_has_no_box_art_and_no_escapes() {
        let plain = render_plain(&grid(), true);
        assert!(!plain.contains('│'));
        assert!(!plain.contains('\u{1b}'));
        assert!(plain.starts_with("A "));
    }

    #[test]
    fn natural_widths_never_truncate_because_they_were_measured_from_the_content() {
        let g = grid();
        let widths = natural_widths(&g, true);
        let rendered = render_plain_with(&g, true, &widths);
        assert!(rendered.contains("a \"quoted\", comma"), "{rendered}");
        assert!(!rendered.contains('…'));
    }

    #[test]
    fn fixed_widths_truncate_so_a_long_cell_cannot_shift_the_columns_after_it() {
        // The streaming (`--all`) case: rows are rendered one at a time against
        // widths chosen up front, so an over-long title must be cut rather than
        // pushing everything right.
        let mut g = Grid::new(vec!["A".into(), "B".into(), "C".into()]);
        g.push(vec![
            GridCell::plain("short"),
            GridCell::plain("x"),
            GridCell::plain("tail"),
        ]);
        g.push(vec![
            GridCell::plain("a title far longer than the column"),
            GridCell::plain("y"),
            GridCell::plain("tail"),
        ]);
        let rendered = render_plain_with(&g, false, &[10, 3, 0]);
        let lines: Vec<&str> = rendered.lines().collect();

        // Column B starts at the same *display* column on both rows. Byte
        // offsets would differ here: the ellipsis is three bytes and one column.
        let column_of = |line: &str, marker: char| {
            let byte = line.find(marker).expect("the marker is present");
            display_width(&line[..byte])
        };
        assert_eq!(
            column_of(lines[0], 'x'),
            column_of(lines[1], 'y'),
            "{rendered}"
        );
        assert!(lines[1].contains('…'), "{rendered}");
        // The last column is never cut.
        assert!(lines[1].ends_with("tail"));
    }

    #[test]
    fn plain_rendering_flattens_embedded_newlines_so_one_row_stays_one_line() {
        let mut g = Grid::new(vec!["A".into(), "B".into()]);
        g.push(vec![GridCell::plain("multi\nline"), GridCell::plain("x")]);
        let plain = render_plain(&g, false);
        assert_eq!(plain.lines().count(), 1, "{plain:?}");
    }

    #[test]
    fn the_boxed_table_emits_no_escapes_when_colour_is_off() {
        let rendered = render_boxed(&grid(), false, 0);
        assert!(rendered.contains('│'));
        assert!(!rendered.contains('\u{1b}'));
    }

    #[test]
    fn truncation_counts_display_width_not_bytes() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        // CJK glyphs are two columns wide each, so a truncation can land one
        // column short — never over, which is what would break the layout.
        assert_eq!(display_width("日本語"), 6);
        let cut = truncate("日本語テスト", 6);
        assert!(display_width(&cut) <= 6, "{cut:?}");
        assert!(cut.ends_with('…'));
    }
}
