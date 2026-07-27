//! Fuzzy matching, and the inline type-to-filter picker built on it.
//!
//! One implementation serves `ovis page list --pick` and the TUI's connector
//! scope overlay, which is what `05_PAGE_NAVIGATION_UX.md` §3 asks for. It
//! renders on **stderr** so `--pick` can still be used in a pipeline without the
//! chooser landing in the data.

use std::io::Write;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, queue, terminal};

use crate::error::{CliError, CliResult};

/// Subsequence match with a score: higher is better.
///
/// Bonuses for matching at a word boundary and for consecutive runs, so
/// `stph` ranks `stanford-philosophy` above a scattered coincidence.
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    let mut score = 0;
    let mut hay_index = 0;
    let mut run = 0;

    for want in needle.to_lowercase().chars() {
        let mut found = None;
        while hay_index < hay.len() {
            if hay[hay_index] == want {
                found = Some(hay_index);
                break;
            }
            hay_index += 1;
            run = 0;
        }
        let at = found?;
        score += 1 + run;
        // A match right after a separator is a word start, and word starts are
        // what people actually type.
        if at == 0
            || matches!(
                hay.get(at.wrapping_sub(1)),
                Some('-' | '_' | ' ' | '/' | '.')
            )
        {
            score += 4;
        }
        run += 2;
        hay_index = at + 1;
    }
    // Shorter haystacks that matched are the tighter fit.
    Some(score * 100 - haystack.len() as i32)
}

/// Rank `labels` against `query`, best first. An empty query keeps the original
/// order rather than shuffling it.
pub fn rank(labels: &[String], query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..labels.len()).collect();
    }
    let mut scored: Vec<(usize, i32)> = labels
        .iter()
        .enumerate()
        .filter_map(|(i, label)| fuzzy_score(query, label).map(|s| (i, s)))
        .collect();
    // Stable within a score, so equal matches keep their listed order.
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| i).collect()
}

const VISIBLE_ROWS: usize = 12;

/// Show an inline picker and return the chosen index into `labels`.
///
/// `Ok(None)` means the user cancelled, which is not an error.
pub fn pick(prompt: &str, labels: &[String]) -> CliResult<Option<usize>> {
    if labels.is_empty() {
        return Ok(None);
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        return Err(CliError::NeedsConfirmation(
            "--pick needs a terminal; use --limit and a plain list in a pipeline".into(),
        ));
    }

    terminal::enable_raw_mode()
        .map_err(|e| CliError::Other(anyhow::anyhow!("cannot enter raw mode: {e}")))?;
    let result = run_picker(prompt, labels);
    // Restore the terminal whatever happened, including a panic further up: the
    // old TUI left terminals in raw mode on any error path.
    let _ = terminal::disable_raw_mode();
    let mut err = std::io::stderr();
    let _ = execute!(err, cursor::Show);
    let _ = writeln!(err);
    result
}

fn run_picker(prompt: &str, labels: &[String]) -> CliResult<Option<usize>> {
    let mut query = String::new();
    let mut cursor_row = 0usize;
    let mut matches = rank(labels, &query);
    let mut drawn_lines = 0usize;
    let mut err = std::io::stderr();

    loop {
        drawn_lines = draw(
            &mut err,
            prompt,
            &query,
            labels,
            &matches,
            cursor_row,
            drawn_lines,
        )?;

        let Event::Key(key) = event::read()
            .map_err(|e| CliError::Other(anyhow::anyhow!("cannot read a key: {e}")))?
        else {
            continue;
        };
        // Windows fires both press and release; only press is an action.
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                clear(&mut err, drawn_lines)?;
                return Ok(None);
            }
            (KeyCode::Enter, _) => {
                clear(&mut err, drawn_lines)?;
                return Ok(matches.get(cursor_row).copied());
            }
            (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                cursor_row = cursor_row.saturating_sub(1);
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                if cursor_row + 1 < matches.len() {
                    cursor_row += 1;
                }
            }
            (KeyCode::Backspace, _) => {
                query.pop();
                matches = rank(labels, &query);
                cursor_row = 0;
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                query.clear();
                matches = rank(labels, &query);
                cursor_row = 0;
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                while query.pop().is_some_and(|c| c.is_whitespace()) {}
                while query
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace())
                {
                    query.pop();
                }
                matches = rank(labels, &query);
                cursor_row = 0;
            }
            (KeyCode::Char(c), m) if m.is_empty() || m == KeyModifiers::SHIFT => {
                query.push(c);
                matches = rank(labels, &query);
                cursor_row = 0;
            }
            _ => {}
        }
    }
}

fn draw(
    err: &mut std::io::Stderr,
    prompt: &str,
    query: &str,
    labels: &[String],
    matches: &[usize],
    cursor_row: usize,
    previous_lines: usize,
) -> CliResult<usize> {
    clear(err, previous_lines)?;

    let visible = VISIBLE_ROWS.min(matches.len());
    // Keep the cursor inside the window when the list is longer than the pane.
    let start = cursor_row.saturating_sub(visible.saturating_sub(1));

    queue!(err, cursor::Hide).map_err(io)?;
    write!(err, "{prompt} {query}\r\n").map_err(io)?;
    for (offset, index) in matches.iter().skip(start).take(visible).enumerate() {
        let marker = if start + offset == cursor_row {
            "▸"
        } else {
            " "
        };
        write!(err, "{marker} {}\r\n", labels[*index]).map_err(io)?;
    }
    if matches.is_empty() {
        write!(err, "  (no matches)\r\n").map_err(io)?;
    }
    err.flush().map_err(io)?;

    Ok(1 + visible.max(if matches.is_empty() { 1 } else { 0 }))
}

fn clear(err: &mut std::io::Stderr, lines: usize) -> CliResult<()> {
    for _ in 0..lines {
        queue!(
            err,
            cursor::MoveToPreviousLine(1),
            terminal::Clear(terminal::ClearType::CurrentLine)
        )
        .map_err(io)?;
    }
    err.flush().map_err(io)?;
    Ok(())
}

fn io(e: std::io::Error) -> CliError {
    CliError::Other(anyhow::anyhow!("terminal write failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Vec<String> {
        [
            "stanford-philosophy",
            "stanford-encyclopedia",
            "tildes",
            "jax-docs",
            "cato-unbound",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn a_subsequence_matches_and_a_non_subsequence_does_not() {
        assert!(fuzzy_score("sph", "stanford-philosophy").is_some());
        assert!(fuzzy_score("zzz", "stanford-philosophy").is_none());
        // An empty needle matches everything, so an empty filter shows the list.
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(fuzzy_score("TILDES", "tildes").is_some());
        assert!(fuzzy_score("tildes", "TILDES").is_some());
    }

    #[test]
    fn word_starts_outrank_scattered_coincidences() {
        // "sp" as two word-initials beats "sp" buried inside one word.
        let word_starts = fuzzy_score("sp", "stanford-philosophy").unwrap();
        let buried = fuzzy_score("sp", "aaaaasaaaaapaaaaa").unwrap();
        assert!(word_starts > buried, "{word_starts} vs {buried}");
    }

    #[test]
    fn an_exact_prefix_ranks_first() {
        let ranked = rank(&labels(), "tild");
        assert_eq!(labels()[ranked[0]], "tildes");
    }

    #[test]
    fn ranking_narrows_to_the_matches_only() {
        let ranked = rank(&labels(), "stanford");
        assert_eq!(ranked.len(), 2);
        for index in ranked {
            assert!(labels()[index].contains("stanford"));
        }
    }

    #[test]
    fn an_empty_query_preserves_the_incoming_order() {
        // Connectors arrive sorted by document count; an empty filter must not
        // reshuffle them.
        assert_eq!(rank(&labels(), ""), vec![0, 1, 2, 3, 4]);
        assert_eq!(rank(&labels(), "   "), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_query_matching_nothing_ranks_nothing() {
        assert!(rank(&labels(), "qqqq").is_empty());
    }

    #[test]
    fn shorter_names_win_ties_because_they_are_the_tighter_fit() {
        let names = vec!["docs".to_string(), "docs-and-more-words".to_string()];
        let ranked = rank(&names, "docs");
        assert_eq!(names[ranked[0]], "docs");
    }
}
