//! The semantic palette, mirroring the web UI: emerald = ok, amber =
//! warn/active, rose = error/destructive, indigo = info.
//!
//! 256-colour indexes rather than truecolour, because every terminal OVIS is
//! ever run in supports them and the palette is only ever six values.

use anstyle::{AnsiColor, Color, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    /// emerald — healthy, succeeded, present
    Ok,
    /// amber — running, paused, needs attention but not broken
    Warn,
    /// rose — failed, destructive
    Error,
    /// indigo — informational emphasis
    Info,
    Dim,
    Bold,
}

impl Tone {
    pub fn ansi(self) -> Style {
        match self {
            Tone::Plain => Style::new(),
            Tone::Ok => Style::new().fg_color(Some(Color::Ansi256(42.into()))),
            Tone::Warn => Style::new().fg_color(Some(Color::Ansi256(214.into()))),
            Tone::Error => Style::new().fg_color(Some(Color::Ansi256(204.into()))),
            Tone::Info => Style::new().fg_color(Some(Color::Ansi256(105.into()))),
            Tone::Dim => Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightBlack))),
            Tone::Bold => Style::new().bold(),
        }
    }

    pub fn comfy(self) -> Option<comfy_table::Color> {
        match self {
            Tone::Plain | Tone::Bold => None,
            Tone::Ok => Some(comfy_table::Color::AnsiValue(42)),
            Tone::Warn => Some(comfy_table::Color::AnsiValue(214)),
            Tone::Error => Some(comfy_table::Color::AnsiValue(204)),
            Tone::Info => Some(comfy_table::Color::AnsiValue(105)),
            Tone::Dim => Some(comfy_table::Color::DarkGrey),
        }
    }

    /// Wrap `text` in this tone's escapes, or return it untouched when colour is
    /// off. Never emits escapes into a pipe.
    pub fn paint(self, text: &str, enabled: bool) -> String {
        if !enabled || self == Tone::Plain {
            return text.to_string();
        }
        let style = self.ansi();
        format!("{style}{text}{style:#}")
    }
}

/// The tone a connector/cc-pair status should be shown in.
pub fn status_tone(status: &str) -> Tone {
    match status.to_ascii_uppercase().as_str() {
        "ACTIVE" | "SUCCESS" => Tone::Ok,
        "INITIAL_INDEXING" | "IN_PROGRESS" | "NOT_STARTED" => Tone::Warn,
        "FAILED" | "INVALID" | "DELETING" => Tone::Error,
        "PAUSED" | "CANCELED" | "CANCELLED" => Tone::Dim,
        _ => Tone::Plain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_off_means_no_escape_bytes_at_all() {
        // The specific defect: escapes leaking into piped output.
        let painted = Tone::Error.paint("boom", false);
        assert_eq!(painted, "boom");
        assert!(!painted.contains('\u{1b}'));
    }

    #[test]
    fn colour_on_wraps_and_resets() {
        let painted = Tone::Ok.paint("ok", true);
        assert!(painted.starts_with('\u{1b}'));
        assert!(painted.ends_with("\u{1b}[0m"));
        assert!(painted.contains("ok"));
    }

    #[test]
    fn statuses_map_to_the_palette_they_mean() {
        assert_eq!(status_tone("ACTIVE"), Tone::Ok);
        assert_eq!(status_tone("active"), Tone::Ok);
        assert_eq!(status_tone("PAUSED"), Tone::Dim);
        assert_eq!(status_tone("FAILED"), Tone::Error);
        assert_eq!(status_tone("INITIAL_INDEXING"), Tone::Warn);
        // An unknown status is rendered, not swallowed or mis-coloured.
        assert_eq!(status_tone("SOMETHING_NEW"), Tone::Plain);
    }
}
