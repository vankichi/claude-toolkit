//! Overview state machine — pure, terminal-free navigation logic.
//!
//! Tracks which panel is selected, whether we're in the overview or drilled
//! into a panel's full view, and the fuzzy-filter input. `on_overview_key`
//! maps a keypress to an [`Action`] the run loop executes (the live store and
//! terminal live in the run loop, not here), so all of this is unit-testable.

use ccstat::model::Category;
use crossterm::event::KeyCode;

/// The three overview panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Now,
    Stats,
    Map,
}

impl Panel {
    /// Cycle order for j/k navigation.
    const ORDER: [Panel; 3] = [Panel::Now, Panel::Stats, Panel::Map];

    fn index(self) -> usize {
        Self::ORDER.iter().position(|&p| p == self).unwrap_or(0)
    }

    fn next(self) -> Panel {
        Self::ORDER[(self.index() + 1) % Self::ORDER.len()]
    }

    fn prev(self) -> Panel {
        Self::ORDER[(self.index() + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// Whether the overview is showing or a panel has been drilled into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Overview,
    Drill(Panel),
}

/// What the run loop should do after an overview keypress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    EnterDrill(Panel),
}

/// Overview navigation + filter state.
#[derive(Debug)]
pub struct App {
    pub mode: Mode,
    pub selected: Panel,
    pub filter: String,
    pub filtering: bool,
    /// Which category the Top-usage chart breaks down (cycled with `c`).
    pub usage_category: Category,
}

impl Default for App {
    fn default() -> Self {
        Self {
            mode: Mode::Overview,
            selected: Panel::Now,
            filter: String::new(),
            filtering: false,
            usage_category: Category::Model,
        }
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle a keypress while in overview mode, returning the [`Action`] for
    /// the run loop. While the filter box is open, keys edit the filter.
    pub fn on_overview_key(&mut self, code: KeyCode) -> Action {
        if self.filtering {
            match code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filtering = false;
                }
                KeyCode::Enter => self.filtering = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            return Action::None;
        }

        match code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            KeyCode::Char('1') => {
                self.selected = Panel::Now;
                Action::None
            }
            KeyCode::Char('2') => {
                self.selected = Panel::Stats;
                Action::None
            }
            KeyCode::Char('3') => {
                self.selected = Panel::Map;
                Action::None
            }
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => {
                self.selected = self.selected.next();
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => {
                self.selected = self.selected.prev();
                Action::None
            }
            KeyCode::Char('/') => {
                self.filtering = true;
                Action::None
            }
            KeyCode::Char('c') => {
                self.usage_category = self.usage_category.next_tab();
                Action::None
            }
            KeyCode::Enter | KeyCode::Char('e') => Action::EnterDrill(self.selected),
            _ => Action::None,
        }
    }

    /// Enter drill-down on the currently selected panel.
    pub fn enter_drill(&mut self) {
        self.mode = Mode::Drill(self.selected);
    }

    /// Return from drill-down to the overview.
    pub fn exit_drill(&mut self) {
        self.mode = Mode::Overview;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_overview_on_now() {
        let app = App::new();
        assert_eq!(app.mode, Mode::Overview);
        assert_eq!(app.selected, Panel::Now);
    }

    #[test]
    fn number_keys_select_panels() {
        let mut app = App::new();
        assert_eq!(app.on_overview_key(KeyCode::Char('2')), Action::None);
        assert_eq!(app.selected, Panel::Stats);
        app.on_overview_key(KeyCode::Char('3'));
        assert_eq!(app.selected, Panel::Map);
        app.on_overview_key(KeyCode::Char('1'));
        assert_eq!(app.selected, Panel::Now);
    }

    #[test]
    fn jk_cycles_panels() {
        let mut app = App::new();
        app.on_overview_key(KeyCode::Char('j'));
        assert_eq!(app.selected, Panel::Stats);
        app.on_overview_key(KeyCode::Char('j'));
        assert_eq!(app.selected, Panel::Map);
        app.on_overview_key(KeyCode::Char('j'));
        assert_eq!(app.selected, Panel::Now); // wraps
        app.on_overview_key(KeyCode::Char('k'));
        assert_eq!(app.selected, Panel::Map); // wraps back
    }

    #[test]
    fn enter_drills_selected_panel() {
        let mut app = App::new();
        app.on_overview_key(KeyCode::Char('2'));
        assert_eq!(
            app.on_overview_key(KeyCode::Enter),
            Action::EnterDrill(Panel::Stats)
        );
    }

    #[test]
    fn q_quits() {
        let mut app = App::new();
        assert_eq!(app.on_overview_key(KeyCode::Char('q')), Action::Quit);
    }

    #[test]
    fn c_cycles_usage_category() {
        let mut app = App::new();
        assert_eq!(app.usage_category, Category::Model);
        app.on_overview_key(KeyCode::Char('c'));
        assert_eq!(app.usage_category, Category::Agent);
    }

    #[test]
    fn slash_starts_filter_and_keys_edit_it() {
        let mut app = App::new();
        app.on_overview_key(KeyCode::Char('/'));
        assert!(app.filtering);
        app.on_overview_key(KeyCode::Char('b'));
        app.on_overview_key(KeyCode::Char('r'));
        assert_eq!(app.filter, "br");
        app.on_overview_key(KeyCode::Backspace);
        assert_eq!(app.filter, "b");
        // Enter keeps the filter but exits editing.
        app.on_overview_key(KeyCode::Enter);
        assert!(!app.filtering);
        assert_eq!(app.filter, "b");
    }

    #[test]
    fn esc_while_filtering_clears() {
        let mut app = App::new();
        app.on_overview_key(KeyCode::Char('/'));
        app.on_overview_key(KeyCode::Char('x'));
        app.on_overview_key(KeyCode::Esc);
        assert!(!app.filtering);
        assert_eq!(app.filter, "");
    }

    #[test]
    fn q_is_literal_while_filtering() {
        let mut app = App::new();
        app.on_overview_key(KeyCode::Char('/'));
        assert_eq!(app.on_overview_key(KeyCode::Char('q')), Action::None);
        assert_eq!(app.filter, "q");
    }

    #[test]
    fn drill_enter_exit_transitions() {
        let mut app = App::new();
        app.selected = Panel::Map;
        app.enter_drill();
        assert_eq!(app.mode, Mode::Drill(Panel::Map));
        app.exit_drill();
        assert_eq!(app.mode, Mode::Overview);
    }
}
