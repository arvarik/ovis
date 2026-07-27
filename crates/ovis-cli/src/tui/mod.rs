pub mod app;
pub mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use crate::models::{ChunkRecord, DocumentRecord};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

pub use app::{ActivePane, App};

/// Launches the interactive Ratatui TUI dashboard.
pub fn run_tui(documents: Vec<DocumentRecord>, chunks: Vec<ChunkRecord>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(documents, chunks);
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("TUI Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if app.should_quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // If modal delete confirmation is active
                if app.delete_confirm.is_some() {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                            app.delete_selected();
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            app.delete_confirm = None;
                        }
                        _ => {}
                    }
                    continue;
                }

                // If editing search input bar
                if app.active_pane == ActivePane::SearchInput {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => {
                            app.active_pane = ActivePane::LeftList;
                        }
                        KeyCode::Backspace => {
                            app.search_query.pop();
                            app.apply_filter();
                        }
                        KeyCode::Char(c) => {
                            app.search_query.push(c);
                            app.apply_filter();
                        }
                        _ => {}
                    }
                    continue;
                }

                // Global hotkeys when not inputting text
                match key.code {
                    KeyCode::Char('q') => {
                        app.should_quit = true;
                    }
                    KeyCode::Char('/') => {
                        app.active_pane = ActivePane::SearchInput;
                    }
                    KeyCode::Esc => {
                        app.search_query.clear();
                        app.apply_filter();
                        app.active_pane = ActivePane::LeftList;
                    }
                    KeyCode::Tab => {
                        app.toggle_pane();
                    }
                    KeyCode::Char('d') => {
                        if let Some(doc) = app.selected_document() {
                            app.delete_confirm = Some(doc.id.clone());
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if app.active_pane == ActivePane::LeftList {
                            app.select_next();
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if app.active_pane == ActivePane::LeftList {
                            app.select_prev();
                        }
                    }
                    KeyCode::Right | KeyCode::Char('n') => {
                        app.next_page();
                    }
                    KeyCode::Left | KeyCode::Char('p') => {
                        app.prev_page();
                    }
                    KeyCode::Enter => {
                        if app.active_pane == ActivePane::LeftList {
                            app.active_pane = ActivePane::RightInspector;
                        }
                    }
                    _ => {}
                }

                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    app.should_quit = true;
                }
            }
        }
    }
}
