//! Pure, testable application state for `ccmap`'s TUI: which category tab
//! is active, the current filter text, and which row is selected. No
//! terminal or rendering dependencies live here (that is wired in a later
//! task) — everything below is plain data and unit-testable logic.

use crate::model::{Item, Kind};

/// The five category tabs, in the fixed order they cycle through.
const TABS: [Kind; 5] = [
    Kind::Agent,
    Kind::Skill,
    Kind::Command,
    Kind::Plugin,
    Kind::Mcp,
];

/// State for the `ccmap` TUI: the full discovered item set, which tab is
/// active, the current filter text, whether filter-entry mode (`/`) is
/// active, and which row of the current tab+filter's visible list is
/// selected.
///
/// `visible` recomputes the filtered list on demand rather than caching it;
/// at the expected item counts (hundreds), an O(n) scan per keystroke is
/// cheap.
pub struct AppState {
    all: Vec<Item>,
    pub tab: Kind,
    pub filter: String,
    pub filtering: bool,
    selected: usize,
}

impl AppState {
    /// Builds a new state from a freshly discovered item set, starting on
    /// the `Agent` tab with no filter and nothing selected.
    #[must_use]
    pub fn new(items: Vec<Item>) -> Self {
        Self {
            all: items,
            tab: Kind::Agent,
            filter: String::new(),
            filtering: false,
            selected: 0,
        }
    }

    /// Items in the active tab whose name or description contains the
    /// current filter text as a case-insensitive substring. An empty
    /// filter matches everything.
    #[must_use]
    pub fn visible(&self) -> Vec<&Item> {
        let needle = self.filter.to_lowercase();
        self.all
            .iter()
            .filter(|item| item.kind == self.tab)
            .filter(|item| {
                needle.is_empty()
                    || item.name.to_lowercase().contains(&needle)
                    || item.description.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// The currently selected item within the visible list, or `None` if
    /// the visible list is empty.
    #[must_use]
    pub fn selected_item(&self) -> Option<&Item> {
        self.visible().into_iter().nth(self.selected)
    }

    /// Moves the selection one row down, clamped to the last visible row
    /// (no wraparound).
    pub fn next(&mut self) {
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        if self.selected + 1 < len {
            self.selected += 1;
        }
    }

    /// Moves the selection one row up, clamped to the first row (no
    /// wraparound).
    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Switches to the next tab in fixed order, wrapping from `Mcp` back to
    /// `Agent`, and resets the selection to the top row.
    pub fn next_tab(&mut self) {
        let idx = TABS.iter().position(|kind| *kind == self.tab).unwrap_or(0);
        self.tab = TABS[(idx + 1) % TABS.len()];
        self.selected = 0;
    }

    /// Switches to the previous tab in fixed order, wrapping from `Agent`
    /// back to `Mcp`, and resets the selection to the top row.
    pub fn prev_tab(&mut self) {
        let idx = TABS.iter().position(|kind| *kind == self.tab).unwrap_or(0);
        self.tab = TABS[(idx + TABS.len() - 1) % TABS.len()];
        self.selected = 0;
    }

    /// Replaces the filter text and resets the selection to the top row.
    pub fn set_filter(&mut self, s: String) {
        self.filter = s;
        self.selected = 0;
    }

    /// Replaces the discovered item set (used by manual rescan) and clamps
    /// the selection so it stays within the new visible list's bounds.
    pub fn reload(&mut self, items: Vec<Item>) {
        self.all = items;
        let len = self.visible().len();
        self.selected = if len == 0 {
            0
        } else {
            self.selected.min(len - 1)
        };
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{Item, Kind, Source};
    use crate::ui::AppState;

    fn item(kind: Kind, name: &str, description: &str) -> Item {
        Item {
            kind,
            name: name.to_string(),
            description: description.to_string(),
            source: Source::User,
            path: None,
            extra: Vec::new(),
            plugin_state: None,
        }
    }

    #[test]
    fn visible_includes_only_items_matching_the_active_tab() {
        let state = AppState::new(vec![
            item(Kind::Agent, "alpha", "first agent"),
            item(Kind::Agent, "beta", "second agent"),
            item(Kind::Skill, "gamma", "a skill"),
        ]);

        let names: Vec<&str> = state
            .visible()
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn visible_filters_by_case_insensitive_substring_over_name_and_description() {
        let mut state = AppState::new(vec![
            item(Kind::Agent, "Alpha", "matches by name"),
            item(Kind::Agent, "Beta", "MATCHES by description"),
            item(Kind::Agent, "Gamma", "no hit here"),
        ]);
        state.set_filter("match".to_string());

        let names: Vec<&str> = state
            .visible()
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(names, vec!["Alpha", "Beta"]);
    }

    #[test]
    fn selection_clamps_at_both_bounds_without_wrapping() {
        let mut state = AppState::new(vec![
            item(Kind::Agent, "one", ""),
            item(Kind::Agent, "two", ""),
            item(Kind::Agent, "three", ""),
        ]);

        state.prev();
        assert_eq!(state.selected_item().unwrap().name, "one");

        state.next();
        state.next();
        assert_eq!(state.selected_item().unwrap().name, "three");

        state.next();
        assert_eq!(state.selected_item().unwrap().name, "three");

        state.prev();
        state.prev();
        assert_eq!(state.selected_item().unwrap().name, "one");

        state.prev();
        assert_eq!(state.selected_item().unwrap().name, "one");
    }

    #[test]
    fn next_tab_cycles_in_fixed_order_wraps_and_resets_selection() {
        let mut state = AppState::new(vec![
            item(Kind::Agent, "agent-1", ""),
            item(Kind::Agent, "agent-2", ""),
            item(Kind::Skill, "skill-1", ""),
            item(Kind::Command, "command-1", ""),
            item(Kind::Plugin, "plugin-1", ""),
            item(Kind::Mcp, "mcp-1", ""),
        ]);
        state.next();
        assert_eq!(state.selected_item().unwrap().name, "agent-2");

        state.next_tab();
        assert_eq!(state.tab, Kind::Skill);
        assert_eq!(state.selected_item().unwrap().name, "skill-1");

        state.next_tab();
        assert_eq!(state.tab, Kind::Command);
        state.next_tab();
        assert_eq!(state.tab, Kind::Plugin);
        state.next_tab();
        assert_eq!(state.tab, Kind::Mcp);
        state.next_tab();
        assert_eq!(state.tab, Kind::Agent);
    }

    #[test]
    fn prev_tab_cycles_backward_wraps_and_resets_selection() {
        let mut state = AppState::new(vec![
            item(Kind::Plugin, "plugin-1", ""),
            item(Kind::Plugin, "plugin-2", ""),
            item(Kind::Mcp, "mcp-1", ""),
            item(Kind::Mcp, "mcp-2", ""),
        ]);
        state.tab = Kind::Mcp;
        state.next();
        assert_eq!(state.selected_item().unwrap().name, "mcp-2");

        state.prev_tab();
        assert_eq!(state.tab, Kind::Plugin);
        assert_eq!(state.selected_item().unwrap().name, "plugin-1");

        state.prev_tab();
        state.prev_tab();
        state.prev_tab();
        assert_eq!(state.tab, Kind::Agent);
        state.prev_tab();
        assert_eq!(state.tab, Kind::Mcp);
    }

    #[test]
    fn set_filter_resets_selection_to_zero() {
        let mut state = AppState::new(vec![
            item(Kind::Agent, "one", ""),
            item(Kind::Agent, "two", ""),
            item(Kind::Agent, "three", ""),
        ]);
        state.next();
        state.next();
        assert_eq!(state.selected_item().unwrap().name, "three");

        state.set_filter("t".to_string());
        assert_eq!(state.selected_item().unwrap().name, "two");
    }

    #[test]
    fn selected_item_is_none_when_the_visible_list_is_empty() {
        let state = AppState::new(vec![item(Kind::Skill, "only-skill", "")]);
        assert_eq!(state.selected_item(), None);
    }

    #[test]
    fn reload_clamps_selection_that_is_now_out_of_range() {
        let mut state = AppState::new(vec![
            item(Kind::Agent, "one", ""),
            item(Kind::Agent, "two", ""),
            item(Kind::Agent, "three", ""),
        ]);
        state.next();
        state.next();
        assert_eq!(state.selected_item().unwrap().name, "three");

        state.reload(vec![item(Kind::Agent, "only", "")]);
        assert_eq!(state.selected_item().unwrap().name, "only");

        state.reload(vec![]);
        assert_eq!(state.selected_item(), None);
    }
}
