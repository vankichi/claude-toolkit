//! Core domain enums for `ccstat`: the usage category tabs and the
//! time-window / sort / project-filter selectors that slice the aggregated
//! usage store.

/// One usage category, each a tab in the TUI. `Ord` follows declaration order
/// (Model < Agent < Skill < Command < Mcp), which the live-activity summary
/// relies on for a stable, tab-ordered listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Model,
    Agent,
    Skill,
    Command,
    Mcp,
}

impl Category {
    /// The tabs in fixed left-to-right order.
    pub const ALL: [Category; 5] = [
        Category::Model,
        Category::Agent,
        Category::Skill,
        Category::Command,
        Category::Mcp,
    ];

    /// The category an extracted usage record counts under, and the name it
    /// is recorded by (an MCP record by its server).
    #[must_use]
    pub fn of(item: &cctk::jsonl::Extracted) -> (Category, &str) {
        use cctk::jsonl::Extracted;
        match item {
            Extracted::Model { name, .. } => (Category::Model, name),
            Extracted::Agent { name } => (Category::Agent, name),
            Extracted::Skill { name } => (Category::Skill, name),
            Extracted::Command { name } => (Category::Command, name),
            Extracted::Mcp { server } => (Category::Mcp, server),
        }
    }

    /// Display label for the tab bar.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Category::Model => "Model",
            Category::Agent => "Agents",
            Category::Skill => "Skills",
            Category::Command => "Commands",
            Category::Mcp => "MCP",
        }
    }

    /// Next tab, wrapping from `Mcp` back to `Model`.
    #[must_use]
    pub fn next_tab(self) -> Category {
        cctk::cycle::next(&Self::ALL, self)
    }

    /// Previous tab, wrapping from `Model` back to `Mcp`.
    #[must_use]
    pub fn prev_tab(self) -> Category {
        cctk::cycle::prev(&Self::ALL, self)
    }
}

/// Time window that scopes the aggregate numbers (count, tokens, cost, ranking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    All,
    Days30,
    Days7,
}

impl Window {
    /// Cycle order for the `w` key.
    pub const ALL: [Window; 3] = [Window::All, Window::Days30, Window::Days7];

    /// Number of days the window spans, or `None` for all-time.
    #[must_use]
    pub fn days(self) -> Option<i64> {
        match self {
            Window::All => None,
            Window::Days30 => Some(30),
            Window::Days7 => Some(7),
        }
    }

    /// Short label for the header.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Window::All => "all",
            Window::Days30 => "30d",
            Window::Days7 => "7d",
        }
    }

    /// Next window in cycle order.
    #[must_use]
    pub fn next(self) -> Window {
        cctk::cycle::next(&Self::ALL, self)
    }
}

/// Ranking sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Count,
    Recency,
    Name,
}

impl SortKey {
    pub const ALL: [SortKey; 3] = [SortKey::Count, SortKey::Recency, SortKey::Name];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Count => "count",
            SortKey::Recency => "recency",
            SortKey::Name => "name",
        }
    }

    #[must_use]
    pub fn next(self) -> SortKey {
        cctk::cycle::next(&Self::ALL, self)
    }
}

/// Project scoping for the aggregate: everything, or a single project label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectFilter {
    All,
    Only(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_tabs_cycle_in_fixed_order_and_wrap() {
        assert_eq!(Category::Model.next_tab(), Category::Agent);
        assert_eq!(Category::Mcp.next_tab(), Category::Model);
        assert_eq!(Category::Model.prev_tab(), Category::Mcp);
        assert_eq!(Category::Agent.prev_tab(), Category::Model);
    }

    #[test]
    fn category_titles() {
        assert_eq!(Category::Model.title(), "Model");
        assert_eq!(Category::Mcp.title(), "MCP");
    }

    #[test]
    fn window_days_and_cycle() {
        assert_eq!(Window::All.days(), None);
        assert_eq!(Window::Days30.days(), Some(30));
        assert_eq!(Window::Days7.days(), Some(7));
        assert_eq!(Window::All.next(), Window::Days30);
        assert_eq!(Window::Days7.next(), Window::All);
    }

    #[test]
    fn sortkey_cycle() {
        assert_eq!(SortKey::Count.next(), SortKey::Recency);
        assert_eq!(SortKey::Name.next(), SortKey::Count);
    }
}
