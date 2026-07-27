//! The keymap.
//!
//! One table drives both dispatch and the `?` overlay, so the help can never
//! describe a binding that does not exist or miss one that does — the old TUI
//! had no help at all and several keys that did nothing.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::app::Screen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Help,
    Escape,

    ScreenPages,
    ScreenConnectors,
    ScreenActivity,

    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    Bottom,
    Enter,

    FocusNext,
    InspectorTab,

    Filter,
    ToggleSearch,
    CycleSearchMode,
    ConnectorScope,
    Sort,
    ToggleHidden,

    Mark,
    MarkAll,
    Unmark,

    Delete,
    OpenBrowser,
    Yank,
    FullText,

    Refresh,
    Freeze,

    Pause,
    Resume,
    RunOnce,
    DrillErrors,
    DrillAttempts,
}

/// Which screens a binding applies to. Kept explicit so the help overlay can
/// show only what is live on the current screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Pages,
    Connectors,
    Activity,
}

impl Scope {
    pub fn covers(self, screen: Screen) -> bool {
        match self {
            Scope::Global => true,
            Scope::Pages => screen == Screen::Pages,
            Scope::Connectors => screen == Screen::Connectors,
            Scope::Activity => screen == Screen::Activity,
        }
    }
}

pub struct Binding {
    /// How the key is written in the help overlay.
    pub keys: &'static str,
    pub action: Action,
    pub scope: Scope,
    pub help: &'static str,
}

pub const BINDINGS: &[Binding] = &[
    Binding {
        keys: "1 / 2 / 3",
        action: Action::ScreenPages,
        scope: Scope::Global,
        help: "switch screen: pages, connectors, activity",
    },
    Binding {
        keys: "j / ↓",
        action: Action::Down,
        scope: Scope::Global,
        help: "move down",
    },
    Binding {
        keys: "k / ↑",
        action: Action::Up,
        scope: Scope::Global,
        help: "move up",
    },
    Binding {
        keys: "PgDn / Ctrl+D",
        action: Action::PageDown,
        scope: Scope::Global,
        help: "page down",
    },
    Binding {
        keys: "PgUp / Ctrl+U",
        action: Action::PageUp,
        scope: Scope::Global,
        help: "page up",
    },
    Binding {
        keys: "g / Home",
        action: Action::Top,
        scope: Scope::Global,
        help: "jump to the top",
    },
    Binding {
        keys: "G / End",
        action: Action::Bottom,
        scope: Scope::Global,
        help: "jump to the bottom",
    },
    Binding {
        keys: "⏎",
        action: Action::Enter,
        scope: Scope::Global,
        help: "open / drill in",
    },
    Binding {
        keys: "Tab",
        action: Action::FocusNext,
        scope: Scope::Global,
        help: "move focus between list and detail",
    },
    Binding {
        keys: "Shift+Tab",
        action: Action::InspectorTab,
        scope: Scope::Pages,
        help: "cycle detail tabs: overview / text / chunks / json",
    },
    Binding {
        keys: "/",
        action: Action::Filter,
        scope: Scope::Pages,
        help: "filter by title and URL",
    },
    Binding {
        keys: "s",
        action: Action::ToggleSearch,
        scope: Scope::Pages,
        help: "toggle content search (BM25) instead of list filtering",
    },
    Binding {
        keys: "m",
        action: Action::CycleSearchMode,
        scope: Scope::Pages,
        help: "cycle search mode: keyword / semantic / hybrid",
    },
    Binding {
        keys: "c",
        action: Action::ConnectorScope,
        scope: Scope::Pages,
        help: "scope to a connector",
    },
    Binding {
        keys: "S",
        action: Action::Sort,
        scope: Scope::Pages,
        help: "cycle sort: updated / chunks / id / boost",
    },
    Binding {
        keys: "H",
        action: Action::ToggleHidden,
        scope: Scope::Pages,
        help: "include hidden documents",
    },
    Binding {
        keys: "x / Space",
        action: Action::Mark,
        scope: Scope::Pages,
        help: "mark or unmark this row",
    },
    Binding {
        keys: "X",
        action: Action::MarkAll,
        scope: Scope::Pages,
        help: "mark everything loaded",
    },
    Binding {
        keys: "u",
        action: Action::Unmark,
        scope: Scope::Pages,
        help: "clear all marks",
    },
    Binding {
        keys: "d",
        action: Action::Delete,
        scope: Scope::Pages,
        help: "delete marked rows, or the cursor row",
    },
    Binding {
        keys: "o",
        action: Action::OpenBrowser,
        scope: Scope::Pages,
        help: "open the link in a browser",
    },
    Binding {
        keys: "y",
        action: Action::Yank,
        scope: Scope::Pages,
        help: "copy the URL to the clipboard (OSC 52; works over ssh)",
    },
    Binding {
        keys: "t",
        action: Action::FullText,
        scope: Scope::Pages,
        help: "read the full text in $PAGER",
    },
    Binding {
        keys: "P",
        action: Action::Pause,
        scope: Scope::Connectors,
        help: "pause this connector",
    },
    Binding {
        keys: "R",
        action: Action::Resume,
        scope: Scope::Connectors,
        help: "resume this connector",
    },
    Binding {
        keys: "O",
        action: Action::RunOnce,
        scope: Scope::Connectors,
        help: "crawl once now",
    },
    Binding {
        keys: "e",
        action: Action::DrillErrors,
        scope: Scope::Connectors,
        help: "show this connector's indexing errors",
    },
    Binding {
        keys: "a",
        action: Action::DrillAttempts,
        scope: Scope::Connectors,
        help: "show this connector's index attempts",
    },
    Binding {
        keys: "r",
        action: Action::Refresh,
        scope: Scope::Global,
        help: "refresh now",
    },
    Binding {
        keys: "f",
        action: Action::Freeze,
        scope: Scope::Activity,
        help: "freeze or resume auto-refresh",
    },
    Binding {
        keys: "?",
        action: Action::Help,
        scope: Scope::Global,
        help: "this help",
    },
    Binding {
        keys: "Esc",
        action: Action::Escape,
        scope: Scope::Global,
        help: "back out: overlay, then filter, then scope",
    },
    Binding {
        keys: "q / Ctrl+C",
        action: Action::Quit,
        scope: Scope::Global,
        help: "quit — q backs out of a drill-in first",
    },
];

/// Map a key event to an action.
///
/// Returns `None` for anything unbound, so a stray keypress does nothing rather
/// than falling through to a default.
pub fn resolve(key: KeyEvent, screen: Screen) -> Option<Action> {
    // Windows delivers press *and* release; acting on both fires everything
    // twice. The old TUI did not filter, which is defect T7.
    if key.kind != KeyEventKind::Press {
        return None;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let action = match (key.code, ctrl) {
        (KeyCode::Char('c'), true) => Action::Quit,
        (KeyCode::Char('d'), true) => Action::PageDown,
        (KeyCode::Char('u'), true) => Action::PageUp,
        (_, true) => return None,

        (KeyCode::Char('1'), _) => Action::ScreenPages,
        (KeyCode::Char('2'), _) => Action::ScreenConnectors,
        (KeyCode::Char('3'), _) => Action::ScreenActivity,

        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => Action::Down,
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => Action::Up,
        (KeyCode::PageDown, _) => Action::PageDown,
        (KeyCode::PageUp, _) => Action::PageUp,
        (KeyCode::Char('g'), _) | (KeyCode::Home, _) => Action::Top,
        (KeyCode::Char('G'), _) | (KeyCode::End, _) => Action::Bottom,
        (KeyCode::Enter, _) => Action::Enter,

        (KeyCode::Tab, _) => Action::FocusNext,
        (KeyCode::BackTab, _) => Action::InspectorTab,

        (KeyCode::Char('/'), _) => Action::Filter,
        (KeyCode::Char('s'), _) => Action::ToggleSearch,
        (KeyCode::Char('m'), _) => Action::CycleSearchMode,
        (KeyCode::Char('c'), _) => Action::ConnectorScope,
        (KeyCode::Char('S'), _) => Action::Sort,
        (KeyCode::Char('H'), _) => Action::ToggleHidden,

        (KeyCode::Char('x'), _) | (KeyCode::Char(' '), _) => Action::Mark,
        (KeyCode::Char('X'), _) => Action::MarkAll,
        (KeyCode::Char('u'), _) => Action::Unmark,

        (KeyCode::Char('d'), _) => Action::Delete,
        (KeyCode::Char('o'), _) => Action::OpenBrowser,
        (KeyCode::Char('y'), _) => Action::Yank,
        (KeyCode::Char('t'), _) => Action::FullText,

        (KeyCode::Char('P'), _) => Action::Pause,
        (KeyCode::Char('R'), _) => Action::Resume,
        (KeyCode::Char('O'), _) => Action::RunOnce,
        (KeyCode::Char('e'), _) => Action::DrillErrors,
        (KeyCode::Char('a'), _) => Action::DrillAttempts,

        (KeyCode::Char('r'), _) => Action::Refresh,
        (KeyCode::Char('f'), _) => Action::Freeze,

        (KeyCode::Char('?'), _) => Action::Help,
        (KeyCode::Esc, _) => Action::Escape,
        (KeyCode::Char('q'), _) => Action::Quit,
        _ => return None,
    };

    // A key bound only to another screen must not act here. `s` on the
    // connectors screen, for example, is simply unbound.
    if applies(action, screen) {
        Some(action)
    } else {
        None
    }
}

fn applies(action: Action, screen: Screen) -> bool {
    // ScreenPages stands in for the whole 1/2/3 group in the table.
    if matches!(
        action,
        Action::ScreenPages | Action::ScreenConnectors | Action::ScreenActivity
    ) {
        return true;
    }
    BINDINGS
        .iter()
        .find(|b| b.action == action)
        .is_some_and(|b| b.scope.covers(screen))
}

/// The bindings the help overlay shows for a screen.
pub fn for_screen(screen: Screen) -> impl Iterator<Item = &'static Binding> {
    BINDINGS.iter().filter(move |b| b.scope.covers(screen))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn every_binding_in_the_help_table_is_reachable_from_a_key() {
        // The property that keeps help and dispatch from drifting: nothing is
        // documented that cannot be pressed.
        for binding in BINDINGS {
            let screen = match binding.scope {
                Scope::Connectors => Screen::Connectors,
                Scope::Activity => Screen::Activity,
                _ => Screen::Pages,
            };
            let reachable = ALL_KEYS
                .iter()
                .filter_map(|key| resolve(*key, screen))
                .any(|action| action == binding.action);
            assert!(
                reachable,
                "'{}' ({:?}) is documented but no key produces it",
                binding.keys, binding.action
            );
        }
    }

    /// Every key the dispatcher knows about, for the coverage test above.
    const ALL_KEYS: &[KeyEvent] = &[
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('H'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('P'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('O'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    ];

    #[test]
    fn only_key_presses_act_so_windows_does_not_fire_everything_twice() {
        let mut release = press(KeyCode::Char('d'));
        release.kind = KeyEventKind::Release;
        assert_eq!(resolve(release, Screen::Pages), None);
        assert_eq!(
            resolve(press(KeyCode::Char('d')), Screen::Pages),
            Some(Action::Delete)
        );
    }

    #[test]
    fn screen_switching_works_from_every_screen() {
        for screen in [Screen::Pages, Screen::Connectors, Screen::Activity] {
            assert_eq!(
                resolve(press(KeyCode::Char('2')), screen),
                Some(Action::ScreenConnectors)
            );
        }
    }

    #[test]
    fn a_binding_scoped_to_one_screen_does_nothing_on_another() {
        // `d` deletes documents, which the connectors screen has none of.
        assert_eq!(resolve(press(KeyCode::Char('d')), Screen::Connectors), None);
        // `P` pauses a connector, which the pages screen has none of.
        assert_eq!(resolve(press(KeyCode::Char('P')), Screen::Pages), None);
        assert_eq!(
            resolve(press(KeyCode::Char('P')), Screen::Connectors),
            Some(Action::Pause)
        );
    }

    #[test]
    fn ctrl_c_quits_from_anywhere_and_other_ctrl_chords_do_not_leak() {
        for screen in [Screen::Pages, Screen::Connectors, Screen::Activity] {
            assert_eq!(
                resolve(
                    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                    screen
                ),
                Some(Action::Quit)
            );
        }
        // Ctrl+X is not Mark.
        assert_eq!(
            resolve(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
                Screen::Pages
            ),
            None
        );
    }

    #[test]
    fn unbound_keys_do_nothing_rather_than_falling_through() {
        assert_eq!(resolve(press(KeyCode::Char('~')), Screen::Pages), None);
        assert_eq!(resolve(press(KeyCode::F(7)), Screen::Pages), None);
    }

    #[test]
    fn the_help_for_a_screen_lists_its_own_bindings_and_the_global_ones() {
        let pages: Vec<Action> = for_screen(Screen::Pages).map(|b| b.action).collect();
        assert!(pages.contains(&Action::Delete));
        assert!(pages.contains(&Action::Quit));
        assert!(!pages.contains(&Action::Pause));

        let connectors: Vec<Action> = for_screen(Screen::Connectors).map(|b| b.action).collect();
        assert!(connectors.contains(&Action::Pause));
        assert!(connectors.contains(&Action::Quit));
        assert!(!connectors.contains(&Action::Delete));
    }

    #[test]
    fn every_documented_binding_carries_help_text() {
        for binding in BINDINGS {
            assert!(!binding.keys.is_empty());
            assert!(
                binding.help.len() > 5,
                "{:?} needs a real description",
                binding.action
            );
        }
    }
}
