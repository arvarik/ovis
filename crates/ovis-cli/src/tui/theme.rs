//! The TUI palette, mirroring the CLI's semantic tones and the web UI's
//! emerald-obsidian identity.

use ratatui::style::Color;

/// emerald — healthy, present, succeeded
pub const OK: Color = Color::Indexed(42);
/// amber — running, paused, needs attention
pub const WARN: Color = Color::Indexed(214);
/// rose — failed, destructive
pub const ERROR: Color = Color::Indexed(204);
/// indigo — interactive emphasis
pub const ACCENT: Color = Color::Indexed(105);
pub const MUTED: Color = Color::DarkGray;

/// The colour a connector or attempt status should be drawn in.
pub fn status(status: &str) -> Color {
    match status.to_ascii_uppercase().as_str() {
        "ACTIVE" | "SUCCESS" | "OK" => OK,
        "INITIAL_INDEXING" | "IN_PROGRESS" | "NOT_STARTED" => WARN,
        "FAILED" | "INVALID" | "DELETING" => ERROR,
        "PAUSED" | "CANCELED" | "CANCELLED" => MUTED,
        _ => Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tui_and_cli_palettes_agree() {
        // Both render the same statuses; disagreeing on colour would be a small
        // but constant confusion when moving between them.
        assert_eq!(status("ACTIVE"), OK);
        assert_eq!(status("FAILED"), ERROR);
        assert_eq!(status("PAUSED"), MUTED);
        assert_eq!(status("IN_PROGRESS"), WARN);
        assert_eq!(status("SOMETHING_NEW"), Color::Reset);
    }

    #[test]
    fn the_palette_uses_the_same_ansi_indexes_as_the_cli() {
        assert_eq!(OK, Color::Indexed(42));
        assert_eq!(WARN, Color::Indexed(214));
        assert_eq!(ERROR, Color::Indexed(204));
        assert_eq!(ACCENT, Color::Indexed(105));
    }
}
