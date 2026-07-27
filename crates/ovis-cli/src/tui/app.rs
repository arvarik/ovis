//! Application state.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use ovis_core::api_types::*;
use ratatui::widgets::TableState;

use super::data::{PagesQuery, SearchQuery};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Pages,
    Connectors,
    Activity,
}

impl Screen {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "pages" | "page" => Some(Screen::Pages),
            "connectors" | "connector" => Some(Screen::Connectors),
            "activity" => Some(Screen::Activity),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Screen::Pages => "Pages",
            Screen::Connectors => "Connectors",
            Screen::Activity => "Activity",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Inspector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectorTab {
    Overview,
    Text,
    Chunks,
    Json,
}

impl InspectorTab {
    pub fn next(self) -> Self {
        match self {
            InspectorTab::Overview => InspectorTab::Text,
            InspectorTab::Text => InspectorTab::Chunks,
            InspectorTab::Chunks => InspectorTab::Json,
            InspectorTab::Json => InspectorTab::Overview,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            InspectorTab::Overview => "overview",
            InspectorTab::Text => "text",
            InspectorTab::Chunks => "chunks",
            InspectorTab::Json => "json",
        }
    }
}

/// What is on top of the screen, if anything. Modal keys never leak to the
/// screen below.
pub enum Overlay {
    None,
    Help,
    /// Editing the filter or the search query.
    Input(InputState),
    Picker(PickerState),
    Confirm(ConfirmState),
}

pub struct InputState {
    pub label: &'static str,
    pub buffer: String,
    /// Restored when the edit is cancelled with Esc.
    pub previous: String,
}

pub struct PickerState {
    pub label: &'static str,
    pub query: String,
    pub labels: Vec<String>,
    /// Parallel to `labels`.
    pub values: Vec<PickerValue>,
    pub matches: Vec<usize>,
    pub cursor: usize,
}

#[derive(Debug, Clone)]
pub enum PickerValue {
    /// (connector_id, cc_pair_id, name)
    Connector(i32, i32, String),
    ClearScope,
}

impl PickerState {
    pub fn refilter(&mut self) {
        self.matches = crate::picker::rank(&self.labels, &self.query);
        self.cursor = 0;
    }

    pub fn selected(&self) -> Option<&PickerValue> {
        self.matches
            .get(self.cursor)
            .and_then(|index| self.values.get(*index))
    }
}

pub struct ConfirmState {
    pub title: String,
    pub lines: Vec<String>,
    pub danger: bool,
    pub on_confirm: PendingAction,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    DeletePages(Vec<String>),
    ConnectorAction {
        cc_pair_id: i32,
        name: String,
        action: super::data::ConnectorAction,
    },
}

/// A transient message under the status bar.
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

impl Toast {
    pub fn expired(&self) -> bool {
        self.at.elapsed() > Duration::from_secs(8)
    }
}

// ---------------------------------------------------------------------------

/// What the pages list is currently showing.
#[derive(Debug, Clone, PartialEq)]
pub enum PagesSource {
    /// `GET /pages` with filters.
    List,
    /// `GET /search` — content search over the chunk index.
    Search,
}

pub struct PagesState {
    pub source: PagesSource,
    pub items: Vec<PageListItem>,
    /// Populated in search mode; parallel to `items`, which is synthesised from
    /// the hits so one renderer serves both.
    pub hits: Vec<SearchHit>,
    pub table: TableState,
    pub total: i64,
    pub total_exact: bool,
    pub next_cursor: Option<String>,
    pub loading: bool,
    pub filter: String,
    pub connector: Option<(i32, i32, String)>,
    pub sort_index: usize,
    pub include_hidden: bool,
    pub search_mode: usize,
    pub degraded: Option<String>,
    pub took_ms: u64,
    pub marks: HashSet<String>,
    pub detail: Option<PageDetail>,
    pub detail_for: Option<String>,
    pub text: Option<String>,
    pub chunks: Option<ChunksResponse>,
    pub tab: InspectorTab,
    pub inspector_scroll: u16,
    /// Set when the selection moves; the fetch fires once it has settled.
    pub pending_detail: Option<(String, Instant)>,
    pub pending_query: Option<Instant>,
}

pub const SORTS: [(&str, &str); 4] = [
    ("updated_desc", "updated↓"),
    ("chunks_desc", "chunks↓"),
    ("id_asc", "id↑"),
    ("boost_desc", "boost↓"),
];

pub const SEARCH_MODES: [&str; 3] = ["keyword", "semantic", "hybrid"];

impl Default for PagesState {
    fn default() -> Self {
        Self {
            source: PagesSource::List,
            items: Vec::new(),
            hits: Vec::new(),
            table: TableState::default(),
            total: 0,
            total_exact: true,
            next_cursor: None,
            loading: false,
            filter: String::new(),
            connector: None,
            sort_index: 0,
            include_hidden: false,
            search_mode: 0,
            degraded: None,
            took_ms: 0,
            marks: HashSet::new(),
            detail: None,
            detail_for: None,
            text: None,
            chunks: None,
            tab: InspectorTab::Overview,
            inspector_scroll: 0,
            pending_detail: None,
            pending_query: None,
        }
    }
}

impl PagesState {
    pub fn selected(&self) -> Option<&PageListItem> {
        self.table.selected().and_then(|i| self.items.get(i))
    }

    pub fn query(&self) -> PagesQuery {
        PagesQuery {
            filter: self.filter.clone(),
            connector_id: self.connector.as_ref().map(|(id, _, _)| *id),
            sort: SORTS[self.sort_index].0.to_string(),
            include_hidden: self.include_hidden,
            cursor: None,
        }
    }

    pub fn search_query(&self) -> SearchQuery {
        SearchQuery {
            q: self.filter.clone(),
            mode: SEARCH_MODES[self.search_mode].to_string(),
            connector_id: self.connector.as_ref().map(|(id, _, _)| *id),
        }
    }

    /// The ids `d` would act on: the marks if any, else the cursor row.
    pub fn delete_targets(&self) -> Vec<String> {
        if !self.marks.is_empty() {
            let mut ids: Vec<String> = self.marks.iter().cloned().collect();
            ids.sort();
            return ids;
        }
        self.selected()
            .map(|i| vec![i.id.clone()])
            .unwrap_or_default()
    }

    /// Drop rows that were deleted, and keep the cursor somewhere sensible.
    pub fn remove(&mut self, ids: &[String]) {
        let removed: HashSet<&String> = ids.iter().collect();
        self.items.retain(|item| !removed.contains(&item.id));
        self.hits.retain(|hit| !removed.contains(&hit.document_id));
        for id in ids {
            self.marks.remove(id);
        }
        self.total = (self.total - ids.len() as i64).max(0);
        if let Some(selected) = self.table.selected() {
            if self.items.is_empty() {
                self.table.select(None);
            } else if selected >= self.items.len() {
                self.table.select(Some(self.items.len() - 1));
            }
        }
        // The detail pane may be showing something that no longer exists.
        if self
            .detail_for
            .as_ref()
            .is_some_and(|id| removed.contains(id))
        {
            self.detail = None;
            self.detail_for = None;
            self.text = None;
            self.chunks = None;
        }
    }
}

#[derive(Default)]
pub struct ConnectorsState {
    pub items: Vec<ConnectorSummary>,
    pub table: TableState,
    pub detail: Option<ConnectorDetail>,
    pub detail_for: Option<i32>,
    pub loading: bool,
    pub pending_detail: Option<(i32, Instant)>,
    /// Set by `e`: the error drill-in for the selected connector.
    pub errors: Option<(i32, Vec<IndexAttemptError>, String)>,
    pub inspector_scroll: u16,
}

impl ConnectorsState {
    pub fn selected(&self) -> Option<&ConnectorSummary> {
        self.table.selected().and_then(|i| self.items.get(i))
    }
}

#[derive(Default)]
pub struct ActivityState {
    pub attempts: Vec<IndexAttemptItem>,
    pub table: TableState,
    pub stats: Option<StatsOverview>,
    pub frozen: bool,
    pub loading: bool,
    /// Non-empty when drilled into one connector's attempts.
    pub scope: Option<String>,
}

pub struct App {
    pub screen: Screen,
    pub focus: Focus,
    pub overlay: Overlay,
    pub pages: PagesState,
    pub connectors: ConnectorsState,
    pub activity: ActivityState,
    pub toast: Option<Toast>,
    pub should_quit: bool,
    /// Set when `t` wants the terminal back for `$PAGER`.
    pub suspend_for_pager: Option<String>,
    pub auto_refresh: Duration,
    pub last_refresh: Instant,
    pub server: String,
}

impl App {
    pub fn new(screen: Screen, auto_refresh_secs: u64, server: String) -> Self {
        Self {
            screen,
            focus: Focus::List,
            overlay: Overlay::None,
            pages: PagesState::default(),
            connectors: ConnectorsState::default(),
            activity: ActivityState::default(),
            toast: None,
            should_quit: false,
            suspend_for_pager: None,
            auto_refresh: Duration::from_secs(auto_refresh_secs.clamp(1, 3600)),
            last_refresh: Instant::now(),
            server,
        }
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    pub fn has_overlay(&self) -> bool {
        !matches!(self.overlay, Overlay::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> PageListItem {
        PageListItem {
            id: id.into(),
            semantic_id: id.into(),
            link: Some(id.into()),
            updated_at: "2026-07-20T00:00:00Z".parse().unwrap(),
            doc_updated_at: None,
            last_modified: "2026-07-20T00:00:00Z".parse().unwrap(),
            chunk_count: Some(1),
            boost: 0,
            hidden: false,
            connector_id: Some(1),
            connector_name: Some("c".into()),
            connector_source: Some("WEB".into()),
            metadata: None,
        }
    }

    fn state_with(ids: &[&str]) -> PagesState {
        let mut state = PagesState {
            items: ids.iter().map(|id| item(id)).collect(),
            total: ids.len() as i64,
            ..Default::default()
        };
        state.table.select(Some(0));
        state
    }

    #[test]
    fn delete_acts_on_the_marks_when_there_are_any_and_the_cursor_otherwise() {
        let mut state = state_with(&["a", "b", "c"]);
        state.table.select(Some(1));
        assert_eq!(state.delete_targets(), vec!["b"]);

        state.marks.insert("a".into());
        state.marks.insert("c".into());
        assert_eq!(state.delete_targets(), vec!["a", "c"]);
    }

    #[test]
    fn deleting_nothing_when_the_list_is_empty_is_an_empty_target_list() {
        let state = PagesState::default();
        assert!(state.delete_targets().is_empty());
    }

    #[test]
    fn removing_rows_drops_them_marks_and_all_and_adjusts_the_total() {
        let mut state = state_with(&["a", "b", "c"]);
        state.marks.insert("b".into());
        state.remove(&["b".to_string()]);
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.total, 2);
        assert!(state.marks.is_empty());
    }

    #[test]
    fn the_cursor_never_dangles_past_the_end_after_a_delete() {
        let mut state = state_with(&["a", "b", "c"]);
        state.table.select(Some(2));
        state.remove(&["c".to_string()]);
        assert_eq!(state.table.selected(), Some(1));

        state.remove(&["a".to_string(), "b".to_string()]);
        assert_eq!(
            state.table.selected(),
            None,
            "an empty list selects nothing"
        );
    }

    #[test]
    fn the_detail_pane_is_cleared_when_its_document_is_deleted() {
        let mut state = state_with(&["a", "b"]);
        state.detail_for = Some("a".into());
        state.text = Some("body".into());
        state.remove(&["a".to_string()]);
        assert!(state.detail_for.is_none());
        assert!(state.text.is_none());
    }

    #[test]
    fn a_delete_that_removes_more_than_the_total_does_not_go_negative() {
        let mut state = state_with(&["a"]);
        state.total = 0;
        state.remove(&["a".to_string()]);
        assert_eq!(state.total, 0);
    }

    #[test]
    fn the_sort_and_mode_cycles_wrap() {
        assert_eq!(SORTS.len(), 4);
        assert_eq!(SEARCH_MODES.len(), 3);
        let mut state = PagesState::default();
        for expected in [1, 2, 3, 0] {
            state.sort_index = (state.sort_index + 1) % SORTS.len();
            assert_eq!(state.sort_index, expected);
        }
        for expected in [1, 2, 0] {
            state.search_mode = (state.search_mode + 1) % SEARCH_MODES.len();
            assert_eq!(state.search_mode, expected);
        }
    }

    #[test]
    fn the_query_carries_the_active_scope_and_sort() {
        let mut state = PagesState {
            filter: "kant".into(),
            connector: Some((291, 5, "jax-docs".into())),
            sort_index: 1,
            ..Default::default()
        };
        let query = state.query();
        assert_eq!(query.filter, "kant");
        // The *connector* id, which is what /pages filters on — not the cc-pair.
        assert_eq!(query.connector_id, Some(291));
        assert_eq!(query.sort, "chunks_desc");
        assert!(!query.include_hidden);

        state.include_hidden = true;
        assert!(state.query().include_hidden);
    }

    #[test]
    fn inspector_tabs_cycle_back_to_the_start() {
        let mut tab = InspectorTab::Overview;
        for expected in [
            InspectorTab::Text,
            InspectorTab::Chunks,
            InspectorTab::Json,
            InspectorTab::Overview,
        ] {
            tab = tab.next();
            assert_eq!(tab, expected);
        }
    }

    #[test]
    fn screens_parse_from_their_config_names() {
        assert_eq!(Screen::parse("pages"), Some(Screen::Pages));
        assert_eq!(Screen::parse("Connectors"), Some(Screen::Connectors));
        assert_eq!(Screen::parse("activity"), Some(Screen::Activity));
        assert_eq!(Screen::parse("nope"), None);
    }

    #[test]
    fn the_picker_narrows_and_reports_what_is_under_the_cursor() {
        let mut picker = PickerState {
            label: "connector",
            query: String::new(),
            labels: vec!["tildes".into(), "jax-docs".into(), "stanford".into()],
            values: vec![
                PickerValue::Connector(1, 1, "tildes".into()),
                PickerValue::Connector(2, 2, "jax-docs".into()),
                PickerValue::Connector(3, 3, "stanford".into()),
            ],
            matches: vec![0, 1, 2],
            cursor: 0,
        };
        picker.query = "jax".into();
        picker.refilter();
        assert_eq!(picker.matches.len(), 1);
        match picker.selected() {
            Some(PickerValue::Connector(_, _, name)) => assert_eq!(name, "jax-docs"),
            other => panic!("expected jax-docs, got {other:?}"),
        }
    }

    #[test]
    fn an_auto_refresh_interval_is_clamped_to_something_sane() {
        // A config typo of 0 would spin the loop; a huge one would never fire.
        assert_eq!(
            App::new(Screen::Pages, 0, "s".into()).auto_refresh,
            Duration::from_secs(1)
        );
        assert_eq!(
            App::new(Screen::Pages, 99_999, "s".into()).auto_refresh,
            Duration::from_secs(3600)
        );
    }
}
