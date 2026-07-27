//! `@N` result handles — the navigation currency of `05_PAGE_NAVIGATION_UX.md`.
//!
//! Every list and search prints a `#` column; `@3` then means "the third row of
//! the last list" to any command that takes an id. The mapping is persisted so
//! it survives between processes, and it expires after an hour so `@3` can never
//! quietly mean a *different* document than the one you looked at.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CliError, CliResult};

/// How long a stored list stays usable. Long enough to browse, short enough
/// that `@3` never silently refers to yesterday's results.
pub const FRESHNESS: Duration = Duration::hours(1);

/// What a stored list is a list *of*. `page view @1` after `connector list`
/// is a mistake worth catching rather than a lookup that half-works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandleKind {
    Page,
    Connector,
}

impl HandleKind {
    fn noun(self) -> &'static str {
        match self {
            HandleKind::Page => "page",
            HandleKind::Connector => "connector",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandleItem {
    pub n: usize,
    /// The id the verb needs: a document id, or a cc-pair id as a string.
    pub id: String,
    /// Human label, so an error can say which row it means.
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandleFile {
    pub created_at: DateTime<Utc>,
    pub kind: HandleKind,
    /// The command that produced the list, echoed back in the staleness error.
    pub command: String,
    pub items: Vec<HandleItem>,
}

pub fn path() -> std::path::PathBuf {
    crate::config::state_dir().join("last-list.json")
}

/// Record a list's rows. Failing to write is never fatal: handles are a
/// convenience, and a read-only state directory should not break `page list`.
pub fn save(kind: HandleKind, command: &str, items: Vec<HandleItem>) {
    if items.is_empty() {
        return;
    }
    let file = HandleFile {
        created_at: Utc::now(),
        kind,
        command: command.to_string(),
        items,
    };
    let path = path();
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Ok(text) = serde_json::to_string(&file) {
        let _ = std::fs::write(&path, text);
    }
}

pub fn load() -> Option<HandleFile> {
    let text = std::fs::read_to_string(path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// `@7` → `Some(7)`. Anything else is a literal id.
pub fn parse(reference: &str) -> Option<usize> {
    reference.strip_prefix('@')?.parse::<usize>().ok()
}

pub fn is_handle(reference: &str) -> bool {
    parse(reference).is_some()
}

/// Resolve one reference: an `@N` handle against the stored list, or a literal
/// id passed straight through.
pub fn resolve(reference: &str, expected: HandleKind) -> CliResult<String> {
    let Some(n) = parse(reference) else {
        return Ok(reference.to_string());
    };

    let Some(file) = load() else {
        return Err(CliError::StaleHandle(format!(
            "{reference} refers to a previous list, but no list has been recorded yet"
        )));
    };

    let age = Utc::now().signed_duration_since(file.created_at);
    if age > FRESHNESS {
        return Err(CliError::StaleHandle(format!(
            "{reference} refers to a list from {} ago, which is past the {}-hour freshness \
             limit. Re-run: {}",
            crate::output::relative_time(&file.created_at, Utc::now()).trim_end_matches(" ago"),
            FRESHNESS.num_hours(),
            file.command
        )));
    }

    if file.kind != expected {
        return Err(CliError::StaleHandle(format!(
            "{reference} refers to a {} list ({}), not a {} list",
            file.kind.noun(),
            file.command,
            expected.noun()
        )));
    }

    match file.items.iter().find(|item| item.n == n) {
        Some(item) => Ok(item.id.clone()),
        None => Err(CliError::StaleHandle(format!(
            "{reference} is out of range: the last list had {} row{}",
            file.items.len(),
            if file.items.len() == 1 { "" } else { "s" }
        ))),
    }
}

/// Resolve a batch, reporting every bad reference at once rather than making
/// the user discover them one run at a time.
pub fn resolve_all(references: &[String], expected: HandleKind) -> CliResult<Vec<String>> {
    let mut resolved = Vec::with_capacity(references.len());
    let mut failures = Vec::new();
    for reference in references {
        match resolve(reference, expected) {
            Ok(id) => resolved.push(id),
            Err(CliError::StaleHandle(msg)) => failures.push(msg),
            Err(other) => return Err(other),
        }
    }
    if !failures.is_empty() {
        return Err(CliError::StaleHandle(failures.join("; ")));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_recognised_and_plain_ids_pass_through() {
        assert_eq!(parse("@3"), Some(3));
        assert_eq!(parse("@0"), Some(0));
        assert_eq!(parse("3"), None);
        assert_eq!(parse("@x"), None);
        // A document id *is* a URL, and one starting with @ would still not
        // parse as a number.
        assert_eq!(parse("@https://example.com"), None);
        assert!(!is_handle("https://example.com/@3"));
    }

    #[test]
    fn resolving_a_literal_id_never_touches_the_state_file() {
        let id = resolve("https://example.com/a", HandleKind::Page).unwrap();
        assert_eq!(id, "https://example.com/a");
    }

    fn file_of(kind: HandleKind, age: Duration) -> HandleFile {
        HandleFile {
            created_at: Utc::now() - age,
            kind,
            command: "ovis page list kant".into(),
            items: vec![
                HandleItem {
                    n: 1,
                    id: "https://example.com/a".into(),
                    label: "A".into(),
                },
                HandleItem {
                    n: 2,
                    id: "https://example.com/b".into(),
                    label: "B".into(),
                },
            ],
        }
    }

    /// The pure half of `resolve`, so the freshness and kind rules are testable
    /// without touching the filesystem.
    fn resolve_against(
        file: &HandleFile,
        reference: &str,
        expected: HandleKind,
    ) -> CliResult<String> {
        let n = parse(reference).unwrap();
        if Utc::now().signed_duration_since(file.created_at) > FRESHNESS {
            return Err(CliError::StaleHandle("stale".into()));
        }
        if file.kind != expected {
            return Err(CliError::StaleHandle("wrong kind".into()));
        }
        file.items
            .iter()
            .find(|i| i.n == n)
            .map(|i| i.id.clone())
            .ok_or_else(|| CliError::StaleHandle("out of range".into()))
    }

    #[test]
    fn a_fresh_handle_resolves_to_its_row() {
        let file = file_of(HandleKind::Page, Duration::minutes(5));
        assert_eq!(
            resolve_against(&file, "@2", HandleKind::Page).unwrap(),
            "https://example.com/b"
        );
    }

    #[test]
    fn an_hour_old_list_is_refused_rather_than_silently_reused() {
        let file = file_of(HandleKind::Page, Duration::minutes(61));
        let err = resolve_against(&file, "@1", HandleKind::Page).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::exit::STALE_HANDLE);
    }

    #[test]
    fn a_handle_from_a_connector_list_will_not_resolve_as_a_page() {
        let file = file_of(HandleKind::Connector, Duration::minutes(1));
        assert!(resolve_against(&file, "@1", HandleKind::Page).is_err());
    }

    #[test]
    fn an_out_of_range_handle_says_so_rather_than_picking_the_last_row() {
        let file = file_of(HandleKind::Page, Duration::minutes(1));
        let err = resolve_against(&file, "@9", HandleKind::Page).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::exit::STALE_HANDLE);
    }

    #[test]
    fn the_freshness_window_is_the_documented_one_hour() {
        assert_eq!(FRESHNESS.num_hours(), 1);
    }
}
