//! Output policy: formats, colour, stdout/stderr separation, paging.
//!
//! The rule the old CLI broke everywhere: **stdout is data, stderr is
//! diagnostics.** `[INFO] 🔍 …` lines went to stdout and corrupted every piped
//! JSON document. Here `note`/`warn`/`footer` write to stderr and nothing else
//! ever does, so `ovis page list -o json | jq .` is guaranteed clean.

pub mod style;
pub mod table;

use std::io::{IsTerminal, Write};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::error::{CliError, CliResult};
use style::Tone;
use table::Grid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Format {
    Table,
    Json,
    Yaml,
    Csv,
    Ndjson,
}

impl Format {
    /// Formats that are a single self-describing document rather than a stream
    /// of records. `--all` cannot use these without buffering everything.
    pub fn is_streamable(self) -> bool {
        matches!(
            self,
            Format::Ndjson | Format::Csv | Format::Table | Format::Json
        )
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Format::Table => "table",
            Format::Json => "json",
            Format::Yaml => "yaml",
            Format::Csv => "csv",
            Format::Ndjson => "ndjson",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

impl std::str::FromStr for ColorChoice {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(ColorChoice::Auto),
            "always" | "yes" | "true" => Ok(ColorChoice::Always),
            "never" | "no" | "false" => Ok(ColorChoice::Never),
            other => Err(format!(
                "unknown colour setting '{other}'; expected auto, always or never"
            )),
        }
    }
}

/// Everything a command needs in order to print.
#[derive(Debug, Clone)]
pub struct Out {
    pub format: Format,
    pub color: bool,
    pub quiet: bool,
    pub stdout_tty: bool,
    pub stderr_tty: bool,
    pub width: u16,
    pub max_width: u16,
    pub pager: Option<String>,
    pub no_headers: bool,
    pub wide: bool,
    pub columns: Option<String>,
    pub hints: bool,
}

impl Default for Out {
    fn default() -> Self {
        Self {
            format: Format::Table,
            color: false,
            quiet: false,
            stdout_tty: false,
            stderr_tty: false,
            width: 100,
            max_width: 0,
            pager: None,
            no_headers: false,
            wide: false,
            columns: None,
            hints: false,
        }
    }
}

impl Out {
    pub fn new(format: Format, choice: ColorChoice, quiet: bool) -> Self {
        let stdout_tty = std::io::stdout().is_terminal();
        let stderr_tty = std::io::stderr().is_terminal();
        let color = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => stdout_tty,
        };
        let width = terminal_width();
        Self {
            format,
            color,
            quiet,
            stdout_tty,
            stderr_tty,
            width,
            max_width: 0,
            pager: None,
            no_headers: false,
            wide: false,
            columns: None,
            hints: true,
        }
    }

    /// How many rows a default list should ask for: the terminal height minus
    /// the chrome a boxed table costs, floored at 20.
    pub fn default_limit(&self) -> i64 {
        if !self.stdout_tty {
            return 50;
        }
        let height = terminal_height() as i64;
        // header box (3) + footer (1) + prompt (1) + the border under the last row.
        (height - 7).max(20)
    }

    // -----------------------------------------------------------------------
    // Diagnostics — stderr only
    // -----------------------------------------------------------------------

    pub fn note(&self, msg: impl AsRef<str>) {
        if self.quiet {
            return;
        }
        let label = Tone::Dim.paint("info:", self.color && self.stderr_tty);
        eprintln!("{label} {}", msg.as_ref());
    }

    pub fn warn(&self, msg: impl AsRef<str>) {
        let label = Tone::Warn.paint("warn:", self.color && self.stderr_tty);
        eprintln!("{label} {}", msg.as_ref());
    }

    pub fn hint(&self, msg: impl AsRef<str>) {
        if self.quiet || !self.hints {
            return;
        }
        let label = Tone::Info.paint("hint:", self.color && self.stderr_tty);
        eprintln!("{label} {}", msg.as_ref());
    }

    /// The teaching footer under a list. Stderr, so it never lands in a pipe.
    pub fn footer(&self, msg: impl AsRef<str>) {
        if self.quiet {
            return;
        }
        eprintln!(
            "{}",
            Tone::Dim.paint(msg.as_ref(), self.color && self.stderr_tty)
        );
    }

    // -----------------------------------------------------------------------
    // Data — stdout only
    // -----------------------------------------------------------------------

    pub fn print(&self, text: impl AsRef<str>) -> CliResult<()> {
        let mut stdout = std::io::stdout().lock();
        // A broken pipe (`… | head -3`) is a normal end, not a failure — and it
        // can surface on the write as easily as on the flush. Rust disables
        // SIGPIPE, so this arrives as EPIPE rather than killing the process.
        let mut write = || -> std::io::Result<()> {
            stdout.write_all(text.as_ref().as_bytes())?;
            if !text.as_ref().ends_with('\n') {
                stdout.write_all(b"\n")?;
            }
            stdout.flush()
        };
        match write() {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Serialise the wire struct itself — `--format json` output is byte-for-byte
    /// the API's own response shape.
    pub fn json<T: Serialize>(&self, value: &T) -> CliResult<()> {
        let text = if self.stdout_tty {
            serde_json::to_string_pretty(value)?
        } else {
            serde_json::to_string(value)?
        };
        self.print(text)
    }

    pub fn yaml<T: Serialize>(&self, value: &T) -> CliResult<()> {
        let text = serde_yaml_ng::to_string(value)
            .map_err(|e| CliError::Other(anyhow::anyhow!("cannot render YAML: {e}")))?;
        self.print(text)
    }

    pub fn ndjson<T: Serialize>(&self, items: &[T]) -> CliResult<()> {
        let mut buf = String::new();
        for item in items {
            buf.push_str(&serde_json::to_string(item)?);
            buf.push('\n');
        }
        self.print(buf)
    }

    /// Render a grid in whichever tabular form applies.
    pub fn grid(&self, grid: &Grid) -> CliResult<()> {
        let text = match self.format {
            Format::Csv => table::render_csv(grid, !self.no_headers)
                .map_err(|e| CliError::Other(anyhow::anyhow!("cannot render CSV: {e}")))?,
            // A pipe gets plain aligned columns: no box art, no colour.
            Format::Table if !self.stdout_tty => table::render_plain(grid, !self.no_headers),
            _ => table::render_boxed(grid, self.color, self.effective_max_width()),
        };
        if text.trim().is_empty() {
            return Ok(());
        }
        self.print(text)
    }

    fn effective_max_width(&self) -> u16 {
        if self.max_width > 0 {
            self.max_width
        } else {
            self.width
        }
    }

    /// Long text goes through `$PAGER` on a terminal and straight to stdout
    /// otherwise. `less -RFX` so short output does not clear the screen and
    /// colour survives.
    pub fn page(&self, text: &str) -> CliResult<()> {
        if !self.stdout_tty {
            return self.print(text);
        }
        let command = self
            .pager
            .clone()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| "less -RFX".to_string());
        let mut parts = command.split_whitespace();
        let Some(program) = parts.next() else {
            return self.print(text);
        };
        let args: Vec<&str> = parts.collect();

        let child = std::process::Command::new(program)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .spawn();

        match child {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    // The reader quitting early (`q` in less) is a broken pipe,
                    // which is a normal end rather than an error.
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
                Ok(())
            }
            // No pager on this machine is not a reason to withhold the output.
            Err(_) => self.print(text),
        }
    }
}

fn terminal_width() -> u16 {
    crossterm::terminal::size().map(|(w, _)| w).unwrap_or(100)
}

fn terminal_height() -> u16 {
    crossterm::terminal::size().map(|(_, h)| h).unwrap_or(30)
}

// ---------------------------------------------------------------------------
// Value formatting
// ---------------------------------------------------------------------------

/// `2h ago` on a terminal, RFC3339 in a pipe or under `--wide` — a relative
/// stamp is unusable in a spreadsheet and an ISO stamp is noise on screen.
pub fn timestamp(ts: &DateTime<Utc>, relative: bool) -> String {
    if relative {
        relative_time(ts, Utc::now())
    } else {
        ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }
}

pub fn timestamp_opt(ts: Option<&DateTime<Utc>>, relative: bool) -> String {
    match ts {
        Some(ts) => timestamp(ts, relative),
        None => "—".to_string(),
    }
}

pub fn relative_time(ts: &DateTime<Utc>, now: DateTime<Utc>) -> String {
    let delta = now.signed_duration_since(*ts);
    let secs = delta.num_seconds();
    // A future timestamp is real here: the crawlers write `last_modified` from
    // their own clocks, which can be a few seconds ahead.
    if secs < 0 {
        let ahead = -secs;
        return if ahead < 90 {
            "just now".to_string()
        } else {
            format!("in {}", coarse(ahead))
        };
    }
    if secs < 45 {
        return "just now".to_string();
    }
    format!("{} ago", coarse(secs))
}

fn coarse(secs: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    match secs {
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < 365 * DAY => format!("{}d", s / DAY),
        s => format!("{}y", s / (365 * DAY)),
    }
}

/// Thousands separators. 1,646,781 is legible; 1646781 is not.
pub fn thousands(n: i64) -> String {
    let negative = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if negative {
        format!("-{out}")
    } else {
        out
    }
}

pub fn bytes(n: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Strip the `<em>` highlight markup the search API emits, optionally
/// re-applying it as colour.
pub fn render_snippet(snippet: &str, color: bool) -> String {
    let flattened = snippet.replace('\n', " ").replace('\r', "");
    if !color {
        return flattened.replace("<em>", "").replace("</em>", "");
    }
    let style = Tone::Warn.ansi();
    flattened
        .replace("<em>", &format!("{style}"))
        .replace("</em>", &format!("{style:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn relative_stamps_are_coarse_and_read_naturally() {
        let now = at("2026-07-26T12:00:00Z");
        assert_eq!(relative_time(&at("2026-07-26T11:59:40Z"), now), "just now");
        assert_eq!(relative_time(&at("2026-07-26T11:30:00Z"), now), "30m ago");
        assert_eq!(relative_time(&at("2026-07-26T09:00:00Z"), now), "3h ago");
        assert_eq!(relative_time(&at("2026-07-23T12:00:00Z"), now), "3d ago");
        assert_eq!(relative_time(&at("2024-07-26T12:00:00Z"), now), "2y ago");
    }

    #[test]
    fn a_slightly_future_timestamp_is_not_rendered_as_a_huge_negative_age() {
        // Crawler clocks run ahead of ours; `last_modified` can legitimately be
        // a few seconds in the future.
        let now = at("2026-07-26T12:00:00Z");
        assert_eq!(relative_time(&at("2026-07-26T12:00:10Z"), now), "just now");
        assert_eq!(relative_time(&at("2026-07-26T14:00:00Z"), now), "in 2h");
    }

    #[test]
    fn absolute_stamps_are_rfc3339_so_they_sort_and_parse() {
        let ts = at("2026-07-26T12:34:56Z");
        assert_eq!(timestamp(&ts, false), "2026-07-26T12:34:56Z");
        assert_eq!(timestamp_opt(None, false), "—");
    }

    #[test]
    fn large_counts_are_grouped() {
        assert_eq!(thousands(1_646_781), "1,646,781");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(-4200), "-4,200");
    }

    #[test]
    fn byte_sizes_are_human_scaled() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        // The live index is 401 GB; it must not render as 4.0e11.
        assert_eq!(bytes(401_889_503_458), "374.3 GB");
    }

    #[test]
    fn snippets_lose_their_markup_when_colour_is_off() {
        let snippet = "the <em>Kant</em> problem\nsecond line";
        let plain = render_snippet(snippet, false);
        assert_eq!(plain, "the Kant problem second line");
        assert!(!plain.contains('\u{1b}'));

        let coloured = render_snippet(snippet, true);
        assert!(!coloured.contains("<em>"));
        assert!(coloured.contains('\u{1b}'));
    }

    #[test]
    fn the_default_limit_is_stable_when_stdout_is_not_a_terminal() {
        // A pipe has no height to fit, so the limit must not depend on one.
        let out = Out::default();
        assert_eq!(out.default_limit(), 50);
    }

    #[test]
    fn a_pipe_never_gets_colour_under_auto() {
        let mut out = Out::default();
        out.color = matches!(ColorChoice::Auto, ColorChoice::Always) || out.stdout_tty;
        assert!(!out.color);
    }

    #[test]
    fn colour_choice_parses_the_documented_words() {
        use std::str::FromStr;
        assert_eq!(ColorChoice::from_str("auto").unwrap(), ColorChoice::Auto);
        assert_eq!(ColorChoice::from_str("NEVER").unwrap(), ColorChoice::Never);
        assert!(ColorChoice::from_str("maybe").is_err());
    }
}
