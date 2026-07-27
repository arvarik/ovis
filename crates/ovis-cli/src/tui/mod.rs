//! The full-screen application.
//!
//! Two properties the old TUI did not have and this one is built around:
//!
//! * **The render loop never waits on the network.** Every fetch goes to the
//!   worker in [`data`] and comes back as an event, so a slow request costs a
//!   spinner rather than a frozen frame.
//! * **Nothing is faked.** Delete calls the API and the row disappears only
//!   after the server says it is gone; the old one mutated a `Vec` and printed
//!   "Successfully deleted page".

pub mod app;
pub mod data;
pub mod keys;
pub mod screens;
pub mod theme;
pub mod widgets;

use std::io::Write;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::cli::TuiArgs;
use crate::ctx::Ctx;
use crate::error::{CliError, CliResult};
use app::{
    App, ConfirmState, Focus, InputState, InspectorTab, Overlay, PagesSource, PendingAction,
    PickerState, PickerValue, Screen, SEARCH_MODES, SORTS,
};
use data::{ConnectorAction, DataEvent, UiCmd};
use keys::Action;

/// Below this the layout has nowhere to go, so the guard screen is honest about
/// it rather than rendering something illegible.
const MIN_SIZE: (u16, u16) = (60, 16);
/// A selection has to settle before its detail is worth fetching.
const DETAIL_DEBOUNCE: Duration = Duration::from_millis(150);
/// Typing has to settle before a server-side query is worth issuing.
const QUERY_DEBOUNCE: Duration = Duration::from_millis(250);

type Term = Terminal<CrosstermBackend<std::io::Stdout>>;

pub async fn run(ctx: &Ctx, args: &TuiArgs) -> CliResult<()> {
    if !ctx.out.stdout_tty {
        return Err(CliError::Usage(
            "the TUI needs a terminal; use `ovis page list` in a pipeline".into(),
        ));
    }

    let screen = match args.screen.as_deref() {
        Some(name) => Screen::parse(name).ok_or_else(|| {
            CliError::Usage(format!(
                "unknown screen '{name}'; expected pages, connectors or activity"
            ))
        })?,
        None => Screen::parse(&ctx.cfg.file.tui.default_screen).unwrap_or(Screen::Pages),
    };

    let mut app = App::new(
        screen,
        ctx.cfg.file.tui.auto_refresh_secs,
        ctx.cfg.server.value.clone(),
    );

    // Launch flags are applied *and shown*: the old TUI's pre-filter was
    // invisible, so `--connector 42` looked like an empty database.
    if let Some(query) = &args.query {
        app.pages.filter = query.clone();
    }
    if let Some(reference) = &args.connector {
        let resolved = crate::resolve::connector(ctx, reference).await?;
        app.pages.connector = Some((
            resolved.connector_id(),
            resolved.cc_pair_id(),
            resolved.name().to_string(),
        ));
    }

    let (cmd_tx, event_rx) = data::spawn(ctx.api.clone());
    let mut terminal = setup()?;
    install_panic_hook();

    let result = event_loop(&mut app, &cmd_tx, event_rx, &mut terminal, ctx).await;

    restore(&mut terminal);
    // Errors surface rather than being swallowed into Ok(()).
    result
}

// ---------------------------------------------------------------------------
// Terminal lifecycle
// ---------------------------------------------------------------------------

fn setup() -> CliResult<Term> {
    enable_raw_mode().map_err(term_err)?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(term_err)?;
    Terminal::new(CrosstermBackend::new(stdout)).map_err(term_err)
}

fn restore(terminal: &mut Term) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
}

/// Leave the terminal usable even on a panic. Without this a crash leaves the
/// shell in raw mode on the alternate screen, with the backtrace invisible.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}

fn term_err(e: std::io::Error) -> CliError {
    CliError::Other(anyhow::anyhow!("terminal error: {e}"))
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

async fn event_loop(
    app: &mut App,
    cmd_tx: &mpsc::Sender<UiCmd>,
    mut event_rx: mpsc::Receiver<DataEvent>,
    terminal: &mut Term,
    ctx: &Ctx,
) -> CliResult<()> {
    let mut keys = crossterm::event::EventStream::new();

    // Everything the first frame needs, in flight before it is drawn.
    let _ = cmd_tx.send(UiCmd::LoadStats).await;
    match app.screen {
        Screen::Pages => reload_pages(app, cmd_tx).await,
        Screen::Connectors => {
            let _ = cmd_tx.send(UiCmd::LoadConnectors).await;
        }
        Screen::Activity => {
            let _ = cmd_tx.send(UiCmd::LoadAttempts).await;
        }
    }

    loop {
        terminal
            .draw(|frame| render(frame, app))
            .map_err(term_err)?;

        if app.should_quit {
            return Ok(());
        }

        // `t` wants the whole terminal for $PAGER, then wants it back.
        if let Some(text) = app.suspend_for_pager.take() {
            restore(terminal);
            let _ = ctx.out.page(&text);
            *terminal = setup()?;
            let _ = terminal.clear();
            continue;
        }

        // Sleep only as long as the nearest deadline, so a debounce fires on
        // time without the loop spinning when nothing is pending.
        let wake = next_wake(app);
        tokio::select! {
            maybe_key = keys.next() => {
                match maybe_key {
                    Some(Ok(Event::Key(key))) => handle_key(app, key, cmd_tx, ctx).await,
                    Some(Ok(Event::Resize(_, _))) => {}
                    Some(Err(e)) => app.error(format!("input error: {e}")),
                    None => return Ok(()),
                    _ => {}
                }
            }
            Some(event) = event_rx.recv() => apply(app, event, cmd_tx).await,
            _ = tokio::time::sleep(wake) => {}
        }

        run_deadlines(app, cmd_tx).await;
    }
}

fn next_wake(app: &App) -> Duration {
    let mut wake = Duration::from_millis(500);
    let mut soonest = |deadline: Instant| {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining < wake {
            wake = remaining.max(Duration::from_millis(10));
        }
    };
    if let Some((_, at)) = &app.pages.pending_detail {
        soonest(*at + DETAIL_DEBOUNCE);
    }
    if let Some(at) = &app.pages.pending_query {
        soonest(*at + QUERY_DEBOUNCE);
    }
    if let Some((_, at)) = &app.connectors.pending_detail {
        soonest(*at + DETAIL_DEBOUNCE);
    }
    if app.screen == Screen::Activity && !app.activity.frozen {
        soonest(app.last_refresh + app.auto_refresh);
    }
    wake
}

/// Fire anything whose debounce or refresh interval has elapsed.
async fn run_deadlines(app: &mut App, cmd_tx: &mpsc::Sender<UiCmd>) {
    if let Some((id, at)) = app.pages.pending_detail.clone() {
        if at.elapsed() >= DETAIL_DEBOUNCE {
            app.pages.pending_detail = None;
            let _ = cmd_tx.send(UiCmd::LoadDetail(id.clone())).await;
            match app.pages.tab {
                InspectorTab::Chunks => {
                    let _ = cmd_tx.send(UiCmd::LoadChunks(id)).await;
                }
                // The overview carries a text preview, so both tabs want it.
                InspectorTab::Overview | InspectorTab::Text => {
                    let _ = cmd_tx.send(UiCmd::LoadText(id)).await;
                }
                InspectorTab::Json => {}
            }
        }
    }

    if let Some(at) = app.pages.pending_query {
        if at.elapsed() >= QUERY_DEBOUNCE {
            app.pages.pending_query = None;
            reload_pages(app, cmd_tx).await;
        }
    }

    if let Some((cc_pair_id, at)) = app.connectors.pending_detail {
        if at.elapsed() >= DETAIL_DEBOUNCE {
            app.connectors.pending_detail = None;
            let _ = cmd_tx.send(UiCmd::LoadConnectorDetail(cc_pair_id)).await;
        }
    }

    if app.screen == Screen::Activity
        && !app.activity.frozen
        && app.last_refresh.elapsed() >= app.auto_refresh
    {
        app.last_refresh = Instant::now();
        let _ = cmd_tx.send(UiCmd::LoadAttempts).await;
        let _ = cmd_tx.send(UiCmd::LoadStats).await;
    }
}

async fn reload_pages(app: &mut App, cmd_tx: &mpsc::Sender<UiCmd>) {
    app.pages.loading = true;
    let cmd = if app.pages.source == PagesSource::Search {
        if app.pages.filter.trim().is_empty() {
            app.pages.loading = false;
            return;
        }
        UiCmd::Search(app.pages.search_query())
    } else {
        UiCmd::LoadPages {
            query: app.pages.query(),
            append: false,
        }
    };
    let _ = cmd_tx.send(cmd).await;
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.area();
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        widgets::too_small(frame, area, MIN_SIZE);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    screens::header(frame, app, rows[0]);
    match app.screen {
        Screen::Pages => screens::pages(frame, app, rows[1]),
        Screen::Connectors => screens::connectors(frame, app, rows[1]),
        Screen::Activity => screens::activity(frame, app, rows[1]),
    }
    screens::status_bar(frame, app, rows[2]);

    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => widgets::help(frame, area, app.screen),
        Overlay::Input(state) => widgets::input(frame, area, state),
        Overlay::Picker(state) => widgets::picker(frame, area, state),
        Overlay::Confirm(state) => widgets::confirm(frame, area, state),
    }
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

async fn handle_key(app: &mut App, key: KeyEvent, cmd_tx: &mpsc::Sender<UiCmd>, ctx: &Ctx) {
    // Windows fires press and release; only press is an action.
    if key.kind != KeyEventKind::Press {
        return;
    }

    // An overlay consumes everything: modal keys never leak to the screen below.
    match &mut app.overlay {
        Overlay::Help => {
            app.overlay = Overlay::None;
            return;
        }
        Overlay::Input(_) => {
            handle_input_key(app, key, cmd_tx).await;
            return;
        }
        Overlay::Picker(_) => {
            handle_picker_key(app, key, cmd_tx).await;
            return;
        }
        Overlay::Confirm(_) => {
            handle_confirm_key(app, key, cmd_tx).await;
            return;
        }
        Overlay::None => {}
    }

    let Some(action) = keys::resolve(key, app.screen) else {
        return;
    };
    dispatch(app, action, cmd_tx, ctx).await;
}

async fn handle_input_key(app: &mut App, key: KeyEvent, cmd_tx: &mpsc::Sender<UiCmd>) {
    let Overlay::Input(state) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match (key.code, ctrl) {
        (KeyCode::Esc, _) => {
            // Cancel restores what the filter was before the edit began.
            let previous = state.previous.clone();
            app.pages.filter = previous;
            app.overlay = Overlay::None;
            app.pages.pending_query = Some(Instant::now());
        }
        (KeyCode::Enter, _) => {
            app.overlay = Overlay::None;
            reload_pages(app, cmd_tx).await;
        }
        (KeyCode::Char('u'), true) => {
            state.buffer.clear();
            app.pages.filter.clear();
            app.pages.pending_query = Some(Instant::now());
        }
        (KeyCode::Char('w'), true) => {
            while state.buffer.pop().is_some_and(|c| c.is_whitespace()) {}
            while state
                .buffer
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace())
            {
                state.buffer.pop();
            }
            app.pages.filter = state.buffer.clone();
            app.pages.pending_query = Some(Instant::now());
        }
        (KeyCode::Backspace, _) => {
            state.buffer.pop();
            app.pages.filter = state.buffer.clone();
            app.pages.pending_query = Some(Instant::now());
        }
        (KeyCode::Char(c), false) => {
            state.buffer.push(c);
            app.pages.filter = state.buffer.clone();
            // Live filtering, debounced — not one request per keystroke.
            app.pages.pending_query = Some(Instant::now());
        }
        _ => {}
    }
}

async fn handle_picker_key(app: &mut App, key: KeyEvent, cmd_tx: &mpsc::Sender<UiCmd>) {
    let Overlay::Picker(state) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match (key.code, ctrl) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => app.overlay = Overlay::None,
        (KeyCode::Up, _) | (KeyCode::Char('p'), true) => {
            state.cursor = state.cursor.saturating_sub(1)
        }
        (KeyCode::Down, _) | (KeyCode::Char('n'), true) => {
            if state.cursor + 1 < state.matches.len() {
                state.cursor += 1;
            }
        }
        (KeyCode::Backspace, _) => {
            state.query.pop();
            state.refilter();
        }
        (KeyCode::Char('u'), true) => {
            state.query.clear();
            state.refilter();
        }
        (KeyCode::Enter, _) => {
            let chosen = state.selected().cloned();
            app.overlay = Overlay::None;
            match chosen {
                Some(PickerValue::Connector(connector_id, cc_pair_id, name)) => {
                    app.pages.connector = Some((connector_id, cc_pair_id, name.clone()));
                    app.toast(format!("scoped to {name}"));
                    reload_pages(app, cmd_tx).await;
                }
                Some(PickerValue::ClearScope) => {
                    app.pages.connector = None;
                    app.toast("scope cleared");
                    reload_pages(app, cmd_tx).await;
                }
                None => {}
            }
        }
        (KeyCode::Char(c), false) => {
            state.query.push(c);
            state.refilter();
        }
        _ => {}
    }
}

async fn handle_confirm_key(app: &mut App, key: KeyEvent, cmd_tx: &mpsc::Sender<UiCmd>) {
    let confirmed = match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => true,
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => false,
        // Enter is deliberately not a yes: a destructive modal should need the
        // letter, not the key people mash to dismiss things.
        _ => return,
    };

    let overlay = std::mem::replace(&mut app.overlay, Overlay::None);
    let Overlay::Confirm(state) = overlay else {
        return;
    };
    if !confirmed {
        app.toast("cancelled");
        return;
    }

    match state.on_confirm {
        PendingAction::DeletePages(ids) => {
            app.toast(format!("deleting {}…", ids.len()));
            let _ = cmd_tx.send(UiCmd::Delete(ids)).await;
        }
        PendingAction::ConnectorAction {
            cc_pair_id,
            name,
            action,
        } => {
            let _ = cmd_tx
                .send(UiCmd::ConnectorAction {
                    cc_pair_id,
                    name,
                    action,
                })
                .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async fn dispatch(app: &mut App, action: Action, cmd_tx: &mpsc::Sender<UiCmd>, ctx: &Ctx) {
    match action {
        Action::Quit => {
            // q backs out of a drill-in first; Ctrl+C always quits, and reaches
            // here only when there is nothing to back out of.
            if app.screen == Screen::Connectors && app.connectors.errors.is_some() {
                app.connectors.errors = None;
            } else if app.screen == Screen::Activity && app.activity.scope.is_some() {
                app.activity.scope = None;
                let _ = cmd_tx.send(UiCmd::LoadAttempts).await;
            } else {
                app.should_quit = true;
            }
        }
        Action::Help => app.overlay = Overlay::Help,
        Action::Escape => escape(app, cmd_tx).await,

        Action::ScreenPages => switch(app, Screen::Pages, cmd_tx).await,
        Action::ScreenConnectors => switch(app, Screen::Connectors, cmd_tx).await,
        Action::ScreenActivity => switch(app, Screen::Activity, cmd_tx).await,

        Action::Down => move_cursor(app, 1, cmd_tx).await,
        Action::Up => move_cursor(app, -1, cmd_tx).await,
        Action::PageDown => move_cursor(app, 10, cmd_tx).await,
        Action::PageUp => move_cursor(app, -10, cmd_tx).await,
        Action::Top => jump(app, true, cmd_tx).await,
        Action::Bottom => jump(app, false, cmd_tx).await,

        Action::FocusNext => {
            app.focus = if app.focus == Focus::List {
                Focus::Inspector
            } else {
                Focus::List
            };
        }
        Action::InspectorTab => {
            app.pages.tab = app.pages.tab.next();
            app.pages.inspector_scroll = 0;
            if let Some(item) = app.pages.selected() {
                let id = item.id.clone();
                match app.pages.tab {
                    InspectorTab::Text if app.pages.text.is_none() => {
                        let _ = cmd_tx.send(UiCmd::LoadText(id)).await;
                    }
                    InspectorTab::Chunks if app.pages.chunks.is_none() => {
                        let _ = cmd_tx.send(UiCmd::LoadChunks(id)).await;
                    }
                    _ => {}
                }
            }
        }

        Action::Enter => enter(app, cmd_tx).await,

        Action::Filter => {
            app.pages.source = PagesSource::List;
            app.overlay = Overlay::Input(InputState {
                label: "filter",
                buffer: app.pages.filter.clone(),
                previous: app.pages.filter.clone(),
            });
        }
        Action::ToggleSearch => {
            app.pages.source = if app.pages.source == PagesSource::Search {
                PagesSource::List
            } else {
                PagesSource::Search
            };
            app.pages.degraded = None;
            app.overlay = Overlay::Input(InputState {
                label: if app.pages.source == PagesSource::Search {
                    "content search"
                } else {
                    "filter"
                },
                buffer: app.pages.filter.clone(),
                previous: app.pages.filter.clone(),
            });
        }
        Action::CycleSearchMode => {
            app.pages.search_mode = (app.pages.search_mode + 1) % SEARCH_MODES.len();
            let mode = SEARCH_MODES[app.pages.search_mode];
            if app.pages.source == PagesSource::Search {
                reload_pages(app, cmd_tx).await;
            } else {
                app.toast(format!("search mode {mode} (press s to search)"));
            }
        }
        Action::ConnectorScope => open_connector_picker(app, ctx).await,
        Action::Sort => {
            app.pages.sort_index = (app.pages.sort_index + 1) % SORTS.len();
            app.toast(format!("sorted {}", SORTS[app.pages.sort_index].1));
            reload_pages(app, cmd_tx).await;
        }
        Action::ToggleHidden => {
            app.pages.include_hidden = !app.pages.include_hidden;
            app.toast(if app.pages.include_hidden {
                "including hidden documents"
            } else {
                "hiding hidden documents"
            });
            reload_pages(app, cmd_tx).await;
        }

        Action::Mark => {
            if let Some(item) = app.pages.selected() {
                let id = item.id.clone();
                if !app.pages.marks.remove(&id) {
                    app.pages.marks.insert(id);
                }
            }
            move_cursor(app, 1, cmd_tx).await;
        }
        Action::MarkAll => {
            app.pages.marks = app.pages.items.iter().map(|i| i.id.clone()).collect();
            app.toast(format!("{} marked", app.pages.marks.len()));
        }
        Action::Unmark => {
            app.pages.marks.clear();
            app.toast("marks cleared");
        }

        Action::Delete => confirm_delete(app),
        Action::OpenBrowser => {
            if let Some(item) = app.pages.selected() {
                let target = item.link.clone().unwrap_or_else(|| item.id.clone());
                match open::that_detached(&target) {
                    Ok(()) => app.toast(format!("opened {target}")),
                    Err(e) => app.error(format!("cannot open a browser: {e}")),
                }
            }
        }
        Action::Yank => {
            if let Some(item) = app.pages.selected() {
                let target = item.link.clone().unwrap_or_else(|| item.id.clone());
                match copy_to_clipboard(&target) {
                    Ok(()) => app.toast(format!("copied {target}")),
                    Err(e) => app.error(format!("cannot copy: {e}")),
                }
            }
        }
        Action::FullText => {
            if let Some(text) = app.pages.text.clone() {
                app.suspend_for_pager = Some(text);
            } else if let Some(id) = app.pages.selected().map(|item| item.id.clone()) {
                app.toast("fetching text…");
                let _ = cmd_tx.send(UiCmd::LoadText(id)).await;
            }
        }

        Action::Refresh => refresh(app, cmd_tx).await,
        Action::Freeze => {
            app.activity.frozen = !app.activity.frozen;
            app.toast(if app.activity.frozen {
                "auto-refresh frozen"
            } else {
                "auto-refresh resumed"
            });
        }

        Action::Pause => connector_action(app, ConnectorAction::Pause, cmd_tx).await,
        Action::Resume => connector_action(app, ConnectorAction::Resume, cmd_tx).await,
        Action::RunOnce => {
            connector_action(
                app,
                ConnectorAction::RunOnce {
                    acknowledge_parked: false,
                },
                cmd_tx,
            )
            .await
        }
        Action::DrillErrors => {
            if let Some(c) = app.connectors.selected() {
                let cc_pair_id = c.cc_pair_id;
                app.toast("loading errors…");
                let _ = cmd_tx.send(UiCmd::LoadConnectorErrors(cc_pair_id)).await;
            }
        }
        Action::DrillAttempts => {
            if let Some(c) = app.connectors.selected() {
                let (cc_pair_id, name) = (c.cc_pair_id, c.name.clone());
                app.activity.scope = Some(name);
                app.screen = Screen::Activity;
                let _ = cmd_tx.send(UiCmd::LoadConnectorAttempts(cc_pair_id)).await;
            }
        }
    }
}

async fn escape(app: &mut App, cmd_tx: &mpsc::Sender<UiCmd>) {
    // Back out one layer at a time: overlay, then filter, then scope.
    if app.has_overlay() {
        app.overlay = Overlay::None;
    } else if !app.pages.filter.is_empty() && app.screen == Screen::Pages {
        app.pages.filter.clear();
        app.pages.source = PagesSource::List;
        reload_pages(app, cmd_tx).await;
    } else if app.pages.connector.is_some() && app.screen == Screen::Pages {
        app.pages.connector = None;
        reload_pages(app, cmd_tx).await;
    } else if app.connectors.errors.is_some() {
        app.connectors.errors = None;
    }
}

async fn switch(app: &mut App, screen: Screen, cmd_tx: &mpsc::Sender<UiCmd>) {
    if app.screen == screen {
        return;
    }
    app.screen = screen;
    app.focus = Focus::List;
    match screen {
        Screen::Pages if app.pages.items.is_empty() => reload_pages(app, cmd_tx).await,
        Screen::Connectors if app.connectors.items.is_empty() => {
            app.connectors.loading = true;
            let _ = cmd_tx.send(UiCmd::LoadConnectors).await;
        }
        Screen::Activity => {
            app.activity.loading = true;
            let _ = cmd_tx.send(UiCmd::LoadAttempts).await;
            let _ = cmd_tx.send(UiCmd::LoadStats).await;
        }
        _ => {}
    }
}

async fn refresh(app: &mut App, cmd_tx: &mpsc::Sender<UiCmd>) {
    app.last_refresh = Instant::now();
    let _ = cmd_tx.send(UiCmd::LoadStats).await;
    match app.screen {
        Screen::Pages => reload_pages(app, cmd_tx).await,
        Screen::Connectors => {
            app.connectors.loading = true;
            let _ = cmd_tx.send(UiCmd::LoadConnectors).await;
        }
        Screen::Activity => {
            let _ = cmd_tx.send(UiCmd::LoadAttempts).await;
        }
    }
}

/// Move the selection, request the new row's detail, and fetch the next keyset
/// page when the cursor nears the end.
async fn move_cursor(app: &mut App, delta: i32, cmd_tx: &mpsc::Sender<UiCmd>) {
    let (len, table) = match app.screen {
        Screen::Pages if app.focus == Focus::Inspector => {
            app.pages.inspector_scroll = app
                .pages
                .inspector_scroll
                .saturating_add_signed(delta.clamp(-30, 30) as i16);
            return;
        }
        Screen::Connectors if app.focus == Focus::Inspector => {
            app.connectors.inspector_scroll = app
                .connectors
                .inspector_scroll
                .saturating_add_signed(delta.clamp(-30, 30) as i16);
            return;
        }
        Screen::Pages => (app.pages.items.len(), &mut app.pages.table),
        Screen::Connectors => (app.connectors.items.len(), &mut app.connectors.table),
        Screen::Activity => (app.activity.attempts.len(), &mut app.activity.table),
    };
    if len == 0 {
        return;
    }
    let current = table.selected().unwrap_or(0) as i32;
    let next = (current + delta).clamp(0, len as i32 - 1) as usize;
    table.select(Some(next));

    after_selection_moved(app, next, len, cmd_tx).await;
}

async fn jump(app: &mut App, top: bool, cmd_tx: &mpsc::Sender<UiCmd>) {
    let (len, table) = match app.screen {
        Screen::Pages => (app.pages.items.len(), &mut app.pages.table),
        Screen::Connectors => (app.connectors.items.len(), &mut app.connectors.table),
        Screen::Activity => (app.activity.attempts.len(), &mut app.activity.table),
    };
    if len == 0 {
        return;
    }
    let index = if top { 0 } else { len - 1 };
    table.select(Some(index));
    after_selection_moved(app, index, len, cmd_tx).await;
}

async fn after_selection_moved(
    app: &mut App,
    index: usize,
    len: usize,
    cmd_tx: &mpsc::Sender<UiCmd>,
) {
    match app.screen {
        Screen::Pages => {
            if let Some(item) = app.pages.items.get(index) {
                let id = item.id.clone();
                if app.pages.detail_for.as_deref() != Some(id.as_str()) {
                    app.pages.detail = None;
                    app.pages.text = None;
                    app.pages.chunks = None;
                    app.pages.inspector_scroll = 0;
                    // Debounced: holding `j` should not fire a request per row.
                    app.pages.pending_detail = Some((id, Instant::now()));
                }
            }
            // Infinite scroll: fetch the next keyset page as the end nears.
            if index + 10 >= len && !app.pages.loading && app.pages.next_cursor.is_some() {
                app.pages.loading = true;
                let mut query = app.pages.query();
                query.cursor = app.pages.next_cursor.clone();
                let _ = cmd_tx
                    .send(UiCmd::LoadPages {
                        query,
                        append: true,
                    })
                    .await;
            }
        }
        Screen::Connectors => {
            if let Some(c) = app.connectors.items.get(index) {
                let cc_pair_id = c.cc_pair_id;
                if app.connectors.detail_for != Some(cc_pair_id) {
                    app.connectors.detail = None;
                    app.connectors.inspector_scroll = 0;
                    app.connectors.pending_detail = Some((cc_pair_id, Instant::now()));
                }
            }
        }
        Screen::Activity => {}
    }
}

async fn enter(app: &mut App, cmd_tx: &mpsc::Sender<UiCmd>) {
    match app.screen {
        // ⏎ on a connector jumps to its documents, which is the drill-down the
        // navigation guide describes.
        Screen::Connectors => {
            if let Some(c) = app.connectors.selected() {
                app.pages.connector = Some((c.connector_id, c.cc_pair_id, c.name.clone()));
                app.pages.source = PagesSource::List;
                app.pages.filter.clear();
                app.screen = Screen::Pages;
                app.focus = Focus::List;
                reload_pages(app, cmd_tx).await;
            }
        }
        Screen::Pages => {
            app.focus = Focus::Inspector;
            if let Some(item) = app.pages.selected() {
                let id = item.id.clone();
                app.pages.pending_detail = Some((id, Instant::now() - DETAIL_DEBOUNCE));
            }
        }
        Screen::Activity => {}
    }
}

async fn open_connector_picker(app: &mut App, ctx: &Ctx) {
    // The connector list is small and cached server-side; fetching it here keeps
    // the picker's contents current without a background poll.
    match ctx.api.connectors().await {
        Ok(mut items) => {
            items.sort_by_key(|c| std::cmp::Reverse(c.doc_count));
            let mut labels = vec!["(clear scope)".to_string()];
            let mut values = vec![PickerValue::ClearScope];
            for c in items {
                labels.push(format!(
                    "{}  {}  {} docs",
                    c.name,
                    c.status,
                    crate::output::thousands(c.doc_count)
                ));
                values.push(PickerValue::Connector(c.connector_id, c.cc_pair_id, c.name));
            }
            let mut state = PickerState {
                label: "connector",
                query: String::new(),
                labels,
                values,
                matches: Vec::new(),
                cursor: 0,
            };
            state.refilter();
            app.overlay = Overlay::Picker(state);
        }
        Err(err) => app.error(format!("cannot load connectors: {}", err.message())),
    }
}

/// Build the delete modal. It shows what will actually happen — the count, the
/// chunks, and whether the connector will simply crawl it back.
fn confirm_delete(app: &mut App) {
    let ids = app.pages.delete_targets();
    if ids.is_empty() {
        app.toast("nothing selected");
        return;
    }

    let chosen: Vec<&ovis_core::api_types::PageListItem> = app
        .pages
        .items
        .iter()
        .filter(|i| ids.contains(&i.id))
        .collect();
    let chunks: i64 = chosen
        .iter()
        .filter_map(|i| i.chunk_count)
        .map(i64::from)
        .sum();
    let at_risk = app
        .pages
        .detail
        .as_ref()
        .filter(|d| ids.contains(&d.item.id) && d.recrawl_risk)
        .is_some();

    let mut lines: Vec<String> = chosen
        .iter()
        .take(8)
        .map(|i| {
            format!(
                "{}  ({} chunks)",
                i.semantic_id,
                i.chunk_count
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".into())
            )
        })
        .collect();
    if chosen.len() > 8 {
        lines.push(format!("… and {} more", chosen.len() - 8));
    }
    lines.push(String::new());
    lines.push(format!(
        "{} document{}, {chunks} chunks in the index.",
        ids.len(),
        if ids.len() == 1 { "" } else { "s" }
    ));
    if at_risk {
        lines
            .push("⚠ the connector is active — the next refresh will likely crawl it back.".into());
    }

    app.overlay = Overlay::Confirm(ConfirmState {
        title: "Delete".into(),
        lines,
        danger: true,
        on_confirm: PendingAction::DeletePages(ids),
    });
}

async fn connector_action(app: &mut App, action: ConnectorAction, cmd_tx: &mpsc::Sender<UiCmd>) {
    let Some(c) = app.connectors.selected() else {
        return;
    };
    let (cc_pair_id, name, parked, docs) = (c.cc_pair_id, c.name.clone(), c.parked, c.doc_count);

    // A parked cc-pair was deliberately finished with by the resilience cron;
    // overriding that is a decision, so the modal explains it rather than the
    // acknowledgement being passed silently.
    if parked && matches!(action, ConnectorAction::RunOnce { .. }) {
        app.overlay = Overlay::Confirm(ConfirmState {
            title: "Parked connector".into(),
            lines: vec![
                format!("{name} is parked: its first-pass crawl is already complete,"),
                "and the resilience cron skips it on purpose.".into(),
                String::new(),
                format!(
                    "Crawl it anyway? ({} documents indexed)",
                    crate::output::thousands(docs)
                ),
            ],
            danger: false,
            on_confirm: PendingAction::ConnectorAction {
                cc_pair_id,
                name,
                action: ConnectorAction::RunOnce {
                    acknowledge_parked: true,
                },
            },
        });
        return;
    }

    app.toast(format!("{} {name}…", action.verb()));
    let _ = cmd_tx
        .send(UiCmd::ConnectorAction {
            cc_pair_id,
            name,
            action,
        })
        .await;
}

/// OSC 52, so a yank works through ssh and tmux where a local clipboard API
/// would not.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut stdout = std::io::stdout();
    write!(stdout, "\x1b]52;c;{encoded}\x07")?;
    stdout.flush()
}

// ---------------------------------------------------------------------------
// Data events
// ---------------------------------------------------------------------------

async fn apply(app: &mut App, event: DataEvent, cmd_tx: &mpsc::Sender<UiCmd>) {
    match event {
        DataEvent::Pages {
            items,
            total,
            total_exact,
            next_cursor,
            append,
        } => {
            app.pages.loading = false;
            app.pages.source = PagesSource::List;
            app.pages.degraded = None;
            app.pages.hits.clear();
            if append {
                app.pages.items.extend(items);
            } else {
                app.pages.items = items;
                app.pages.table.select(if app.pages.items.is_empty() {
                    None
                } else {
                    Some(0)
                });
                if let Some(item) = app.pages.items.first() {
                    app.pages.pending_detail = Some((item.id.clone(), Instant::now()));
                }
            }
            app.pages.total = total;
            app.pages.total_exact = total_exact;
            app.pages.next_cursor = next_cursor;
        }

        DataEvent::SearchResults {
            items,
            total,
            total_exact,
            mode,
            degraded,
            took_ms,
        } => {
            app.pages.loading = false;
            app.pages.source = PagesSource::Search;
            // Hits are projected onto list items so one renderer serves both,
            // and the score/snippet ride alongside.
            app.pages.items = items.iter().map(hit_as_item).collect();
            app.pages.hits = items;
            app.pages.total = total;
            app.pages.total_exact = total_exact;
            app.pages.next_cursor = None;
            app.pages.took_ms = took_ms;
            app.pages.degraded = degraded.clone();
            app.pages.table.select(if app.pages.items.is_empty() {
                None
            } else {
                Some(0)
            });
            if let Some(item) = app.pages.items.first() {
                app.pages.pending_detail = Some((item.id.clone(), Instant::now()));
            }
            if let Some(reason) = degraded {
                if mode != "keyword" {
                    app.error(format!(
                        "{mode} degraded to keyword: {}",
                        crate::commands::search::explain_degraded(&reason)
                    ));
                }
            }
        }

        DataEvent::Detail(detail) => {
            app.pages.detail_for = Some(detail.item.id.clone());
            app.pages.detail = Some(*detail);
        }
        DataEvent::Text(id, text) => {
            if app.pages.selected().map(|i| i.id.as_str()) == Some(id.as_str()) {
                app.pages.text = Some(text);
            }
        }
        DataEvent::Chunks(id, chunks) => {
            if app.pages.selected().map(|i| i.id.as_str()) == Some(id.as_str()) {
                app.pages.chunks = Some(*chunks);
            }
        }

        DataEvent::Connectors(items) => {
            app.connectors.loading = false;
            app.connectors.items = items;
            if app.connectors.table.selected().is_none() && !app.connectors.items.is_empty() {
                app.connectors.table.select(Some(0));
                if let Some(c) = app.connectors.items.first() {
                    app.connectors.pending_detail = Some((c.cc_pair_id, Instant::now()));
                }
            }
        }
        DataEvent::ConnectorDetail(detail) => {
            app.connectors.detail_for = Some(detail.summary.cc_pair_id);
            app.connectors.detail = Some(*detail);
        }
        DataEvent::ConnectorErrors(cc_pair_id, errors, window) => {
            let count = errors.len();
            app.connectors.errors = Some((cc_pair_id, errors, window));
            app.connectors.inspector_scroll = 0;
            app.focus = Focus::Inspector;
            app.toast(format!("{count} errors in the rolling window"));
        }

        DataEvent::Attempts(attempts) => {
            app.activity.loading = false;
            app.activity.attempts = attempts;
            if app.activity.table.selected().is_none() && !app.activity.attempts.is_empty() {
                app.activity.table.select(Some(0));
            }
        }
        DataEvent::Stats(stats) => app.activity.stats = Some(*stats),

        DataEvent::Deleted(response) => {
            // The rows go only now, and only for the ids the server confirmed.
            let failed: std::collections::HashSet<&str> =
                response.failed.iter().map(|f| f.id.as_str()).collect();
            let removed: Vec<String> = app
                .pages
                .delete_targets()
                .into_iter()
                .filter(|id| !failed.contains(id.as_str()))
                .collect();
            app.pages.remove(&removed);

            let mut message = format!(
                "deleted {} · {} chunks",
                response.deleted, response.chunks_deleted
            );
            if response.index_cleanup_pending > 0 {
                message.push_str(&format!(
                    " · {} index cleanups queued for retry",
                    response.index_cleanup_pending
                ));
            }
            if response.failed.is_empty() {
                app.toast(message);
            } else {
                message.push_str(&format!(
                    " · FAILED {} ({})",
                    response.failed.len(),
                    response
                        .failed
                        .iter()
                        .take(2)
                        .map(|f| format!("{}: {}", f.id, f.code))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                app.error(message);
            }
        }

        DataEvent::ActionDone(response) => {
            app.toast(format!(
                "{} → {}{}",
                response.action,
                response.status.clone().unwrap_or_else(|| "ok".into()),
                response
                    .detail
                    .as_deref()
                    .map(|d| format!(" ({})", d.trim()))
                    .unwrap_or_default()
            ));
            // Refetch rather than assume: the status shown must be the one the
            // server reports.
            let _ = cmd_tx.send(UiCmd::LoadConnectors).await;
            let _ = cmd_tx
                .send(UiCmd::LoadConnectorDetail(response.cc_pair_id))
                .await;
        }

        DataEvent::Failed(message) => app.error(message),
    }
}

/// Project a search hit onto a list item so the pages table renders both.
fn hit_as_item(hit: &ovis_core::api_types::SearchHit) -> ovis_core::api_types::PageListItem {
    let updated = hit.updated_at.unwrap_or_else(chrono::Utc::now);
    ovis_core::api_types::PageListItem {
        id: hit.document_id.clone(),
        semantic_id: hit
            .semantic_id
            .clone()
            .unwrap_or_else(|| hit.document_id.clone()),
        link: hit.link.clone(),
        updated_at: updated,
        doc_updated_at: None,
        last_modified: updated,
        chunk_count: hit.chunk_count,
        boost: 0,
        hidden: false,
        connector_id: hit.connector_id,
        connector_name: hit.connector_name.clone(),
        connector_source: hit.connector_source.clone(),
        metadata: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ovis_core::api_types::SearchHit;

    #[test]
    fn a_search_hit_projects_onto_a_list_item_without_inventing_fields() {
        let hit = SearchHit {
            document_id: "https://x/y".into(),
            semantic_id: Some("Title".into()),
            link: Some("https://x/y".into()),
            score: 13.3,
            snippet: Some("<em>kant</em>".into()),
            chunk_index: Some(0),
            connector_id: Some(4),
            connector_name: Some("tildes".into()),
            connector_source: Some("WEB".into()),
            chunk_count: Some(2),
            updated_at: Some("2026-07-20T00:00:00Z".parse().unwrap()),
        };
        let item = hit_as_item(&hit);
        assert_eq!(item.id, "https://x/y");
        assert_eq!(item.semantic_id, "Title");
        assert_eq!(item.chunk_count, Some(2));
        assert_eq!(item.connector_id, Some(4));
        // Search returns no boost or hidden state, so neither is claimed as
        // meaningful — they render as the neutral default.
        assert_eq!(item.boost, 0);
        assert!(!item.hidden);
        assert_eq!(item.doc_updated_at, None);
    }

    #[test]
    fn a_hit_with_no_title_falls_back_to_its_id_rather_than_rendering_blank() {
        let hit = SearchHit {
            document_id: "https://x/y".into(),
            semantic_id: None,
            link: None,
            score: 1.0,
            snippet: None,
            chunk_index: None,
            connector_id: None,
            connector_name: None,
            connector_source: None,
            chunk_count: None,
            updated_at: None,
        };
        assert_eq!(hit_as_item(&hit).semantic_id, "https://x/y");
    }

    #[test]
    fn osc52_wraps_base64_in_the_documented_escape() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode("https://x/y");
        let expected = format!("\x1b]52;c;{encoded}\x07");
        assert!(expected.starts_with("\x1b]52;c;"));
        assert!(expected.ends_with('\x07'));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
            b"https://x/y"
        );
    }

    #[test]
    fn the_minimum_size_guard_matches_the_specification() {
        assert_eq!(MIN_SIZE, (60, 16));
    }

    #[test]
    fn the_debounces_are_the_specified_ones() {
        assert_eq!(DETAIL_DEBOUNCE, Duration::from_millis(150));
        assert_eq!(QUERY_DEBOUNCE, Duration::from_millis(250));
    }
}
