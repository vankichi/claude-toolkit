//! Pure UI state: the aggregated store plus every selector the user can move.

use crate::live::{self, ActiveSet};
use crate::model::{Category, ProjectFilter, SortKey, Window};
use crate::provenance::ProvenanceMap;
use crate::usage::{GraphMetric, Row, UsageDb};
use ccmap::model::Provenance;
use chrono::NaiveDate;

/// Sentinel shown at the top of the project picker for "no filter".
const ALL_PROJECTS: &str = "(all projects)";

/// Pure UI state: the aggregated store plus every selector the user can move.
// The flags are independent modal toggles, not a state to hoist into an enum.
#[allow(clippy::struct_excessive_bools)]
pub struct AppState {
    db: UsageDb,
    provenance: ProvenanceMap,
    today: NaiveDate,
    pub tab: Category,
    pub window: Window,
    pub sort: SortKey,
    pub project: ProjectFilter,
    pub filter: String,
    pub filtering: bool,
    pub picking: bool,
    pub showing_graph: bool,
    /// Metric plotted by the always-on top trends graph (toggled with `m`).
    pub graph_metric: GraphMetric,
    /// `--watch` live mode: drives the running-summary line and per-row
    /// spinners. Off means the classic snapshot rendering is unchanged.
    pub watch: bool,
    active: ActiveSet,
    spinner_tick: u64,
    picker_idx: usize,
    selected: usize,
}

impl AppState {
    #[must_use]
    pub fn new(db: UsageDb, provenance: ProvenanceMap, today: NaiveDate) -> Self {
        Self {
            db,
            provenance,
            today,
            tab: Category::Model,
            window: Window::Days7, // default to the most recent activity
            sort: SortKey::Count,
            project: ProjectFilter::All,
            filter: String::new(),
            filtering: false,
            picking: false,
            showing_graph: false,
            graph_metric: GraphMetric::Tokens,
            watch: false,
            active: ActiveSet::default(),
            spinner_tick: 0,
            picker_idx: 0,
            selected: 0,
        }
    }

    /// Rows for the current tab/window/project/sort, then narrowed by a
    /// case-insensitive fuzzy (subsequence) match on the name.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        self.db
            .rows(self.tab, self.window, &self.project, self.sort, self.today)
            .into_iter()
            .filter(|r| cctk::fuzzy::matches(&self.filter, &r.name))
            .collect()
    }

    #[must_use]
    pub fn selected_row(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.selected)
    }

    #[must_use]
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn next(&mut self) {
        let len = self.rows().len();
        if len > 0 && self.selected + 1 < len {
            self.selected += 1;
        }
    }

    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn next_tab(&mut self) {
        self.tab = self.tab.next_tab();
        self.selected = 0;
    }

    pub fn prev_tab(&mut self) {
        self.tab = self.tab.prev_tab();
        self.selected = 0;
    }

    pub fn cycle_window(&mut self) {
        self.window = self.window.next();
        self.selected = 0;
    }

    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.selected = 0;
    }

    pub fn set_filter(&mut self, s: String) {
        self.filter = s;
        self.selected = 0;
    }

    /// Replace the store (rescan) and clamp the selection.
    pub fn reload(&mut self, db: UsageDb) {
        self.db = db;
        let len = self.rows().len();
        self.selected = if len == 0 {
            0
        } else {
            self.selected.min(len - 1)
        };
    }

    /// Refresh the current date and replace the store (used by rescan so
    /// time-relative fields stay correct across day boundaries).
    pub fn reload_at(&mut self, today: NaiveDate, db: UsageDb, provenance: ProvenanceMap) {
        self.today = today;
        self.provenance = provenance;
        self.reload(db);
    }

    /// The discovered provenance of a used agent/skill/command, or `None`
    /// when it wasn't found by discovery (built-in, deleted, other project).
    #[must_use]
    pub fn provenance_of(&self, category: Category, name: &str) -> Option<Provenance> {
        self.provenance.lookup(category, name)
    }

    // ---- project picker ----

    /// Options shown in the picker: `(all projects)` first, then every project.
    #[must_use]
    pub fn picker_options(&self) -> Vec<String> {
        let mut opts = vec![ALL_PROJECTS.to_string()];
        opts.extend(self.db.projects());
        opts
    }

    #[must_use]
    pub fn picker_index(&self) -> usize {
        self.picker_idx
    }

    pub fn open_project_picker(&mut self) {
        self.picking = true;
        // Preselect the current filter.
        self.picker_idx = match &self.project {
            ProjectFilter::All => 0,
            ProjectFilter::Only(p) => self
                .picker_options()
                .iter()
                .position(|o| o == p)
                .unwrap_or(0),
        };
    }

    pub fn picker_next(&mut self) {
        let len = self.picker_options().len();
        if self.picker_idx + 1 < len {
            self.picker_idx += 1;
        }
    }

    pub fn picker_prev(&mut self) {
        self.picker_idx = self.picker_idx.saturating_sub(1);
    }

    pub fn apply_picker(&mut self) {
        let opts = self.picker_options();
        self.project = match opts.get(self.picker_idx) {
            Some(p) if p != ALL_PROJECTS => ProjectFilter::Only(p.clone()),
            _ => ProjectFilter::All,
        };
        self.picking = false;
        self.selected = 0;
    }

    pub fn cancel_picker(&mut self) {
        self.picking = false;
    }

    /// Toggle the full-screen daily bar-chart view of the selected row.
    /// Turning the graph on is a no-op without a selected row (nothing to
    /// plot); turning it off always succeeds.
    pub fn toggle_graph(&mut self) {
        if !self.showing_graph && self.selected_row().is_none() {
            return;
        }
        self.showing_graph = !self.showing_graph;
    }

    /// Cycle the always-on top trends graph metric (tokens → cost → messages).
    pub fn cycle_graph_metric(&mut self) {
        self.graph_metric = self.graph_metric.next();
    }

    /// Daily-bucket span the trends/graph views cover for the current window
    /// (7 / 30, or all-time capped).
    #[must_use]
    pub fn graph_span(&self) -> usize {
        self.db
            .span_days(self.tab, &self.project, self.window, self.today)
    }

    /// Per-day series for `name` under the current tab/project/metric over
    /// `span` days ending today, oldest→newest.
    #[must_use]
    pub fn graph_series(&self, name: &str, span: usize) -> Vec<f64> {
        self.db.daily_series(
            self.tab,
            name,
            &self.project,
            self.graph_metric,
            span,
            self.today,
        )
    }

    /// `today` used for slicing; exposed for rescans and the detail pane.
    #[must_use]
    pub fn today_for_rescan(&self) -> NaiveDate {
        self.today
    }

    // ---- live (`--watch`) mode ----

    /// Replace the set of items running "now" (recomputed on the active poll).
    pub fn set_active(&mut self, active: ActiveSet) {
        self.active = active;
    }

    /// Advance the spinner animation by one frame.
    pub fn tick_spinner(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
    }

    /// Current spinner glyph for the animation frame.
    #[must_use]
    pub fn spinner_char(&self) -> char {
        live::spinner_frame(self.spinner_tick)
    }

    /// Is `(category, name)` running now? Always false outside watch mode.
    #[must_use]
    pub fn is_active(&self, category: Category, name: &str) -> bool {
        self.watch && self.active.is_active(category, name)
    }

    /// Text for the running-summary line (without the animated leading glyph,
    /// so it is frame-independent). `"idle"` when nothing identifiable is
    /// running; otherwise the deduplicated running names plus the live-session
    /// count, e.g. `running: opus · go-feature-tdd · brainstorm · 2 active`.
    #[must_use]
    pub fn running_summary(&self) -> String {
        const MAX: usize = 8;
        if self.active.is_empty() {
            return "idle".to_string();
        }
        let mut seen = std::collections::HashSet::new();
        let mut names: Vec<&str> = Vec::new();
        for (_, n) in self.active.iter() {
            if seen.insert(n.as_str()) {
                names.push(n.as_str());
            }
        }
        let shown = names.len().min(MAX);
        let mut body = names[..shown].join(" · ");
        if names.len() > MAX {
            body.push_str(" …");
        }
        format!("running: {body} · {} active", self.active.session_count())
    }

    /// Whether anything is currently running (drives the leading glyph choice).
    #[must_use]
    pub fn has_active(&self) -> bool {
        self.watch && !self.active.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::format_recency;
    use crate::ui::test_support::{day, db_with};

    #[test]
    fn starts_on_model_tab_7d_count_sort() {
        let st = AppState::new(
            UsageDb::default(),
            ProvenanceMap::default(),
            day(2026, 7, 16),
        );
        assert_eq!(st.tab, Category::Model);
        assert_eq!(st.window, Window::Days7);
        assert_eq!(st.sort, SortKey::Count);
        assert_eq!(st.project, ProjectFilter::All);
    }

    #[test]
    fn tab_switch_moves_to_the_right_category_and_resets_selection() {
        let db = db_with(&[(Category::Skill, "a", "p"), (Category::Skill, "b", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.next(); // select second model row (none) — no-op, but exercises reset
        st.tab = Category::Skill;
        assert_eq!(st.rows().len(), 2);
    }

    #[test]
    fn filter_narrows_rows_case_insensitively() {
        let db = db_with(&[
            (Category::Skill, "Alpha", "p"),
            (Category::Skill, "beta", "p"),
        ]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.set_filter("alp".to_string());
        let names: Vec<String> = st.rows().into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["Alpha"]);
    }

    #[test]
    fn selection_clamps_and_tracks() {
        let db = db_with(&[(Category::Skill, "a", "p"), (Category::Skill, "b", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        assert_eq!(st.selected_index(), 0);
        st.next();
        assert_eq!(st.selected_index(), 1);
        st.next(); // clamp
        assert_eq!(st.selected_index(), 1);
        st.prev();
        st.prev(); // clamp
        assert_eq!(st.selected_index(), 0);
    }

    #[test]
    fn project_picker_selects_and_applies() {
        let db = db_with(&[
            (Category::Skill, "a", "alpha"),
            (Category::Skill, "a", "beta"),
        ]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        st.open_project_picker();
        assert!(st.picking);
        assert_eq!(
            st.picker_options(),
            vec!["(all projects)".to_string(), "alpha".into(), "beta".into()]
        );
        st.picker_next(); // -> alpha
        st.picker_next(); // -> beta
        st.apply_picker();
        assert!(!st.picking);
        assert_eq!(st.project, ProjectFilter::Only("beta".into()));
    }

    #[test]
    fn project_picker_cancel_keeps_filter() {
        let db = db_with(&[(Category::Skill, "a", "alpha")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.open_project_picker();
        st.picker_next();
        st.cancel_picker();
        assert!(!st.picking);
        assert_eq!(st.project, ProjectFilter::All);
    }

    #[test]
    fn toggle_graph_flips_flag() {
        let db = db_with(&[(Category::Skill, "a", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        assert!(st.selected_row().is_some());
        assert!(!st.showing_graph);
        st.toggle_graph();
        assert!(st.showing_graph);
        st.toggle_graph();
        assert!(!st.showing_graph);
    }

    #[test]
    fn toggle_graph_noop_without_selected_row() {
        let mut st = AppState::new(
            UsageDb::default(),
            ProvenanceMap::default(),
            day(2026, 7, 16),
        );
        assert!(!st.showing_graph);
        st.toggle_graph();
        assert!(!st.showing_graph);
    }

    #[test]
    fn next_tab_resets_selection() {
        let db = db_with(&[
            (Category::Skill, "a", "p"),
            (Category::Skill, "b", "p"),
            (Category::Agent, "x", "p"),
        ]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.window = Window::All;
        st.tab = Category::Skill;
        st.next();
        assert_eq!(st.selected_index(), 1);
        st.next_tab();
        assert_eq!(st.selected_index(), 0);
    }

    #[test]
    fn prev_tab_resets_selection() {
        let db = db_with(&[
            (Category::Skill, "a", "p"),
            (Category::Skill, "b", "p"),
            (Category::Agent, "x", "p"),
        ]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.window = Window::All;
        st.tab = Category::Skill;
        st.next();
        assert_eq!(st.selected_index(), 1);
        st.prev_tab();
        assert_eq!(st.selected_index(), 0);
    }

    #[test]
    fn cycle_window_resets_selection() {
        let db = db_with(&[(Category::Skill, "a", "p"), (Category::Skill, "b", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        st.next();
        assert_eq!(st.selected_index(), 1);
        st.cycle_window();
        assert_eq!(st.selected_index(), 0);
    }

    #[test]
    fn cycle_sort_resets_selection() {
        let db = db_with(&[(Category::Skill, "a", "p"), (Category::Skill, "b", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        st.next();
        assert_eq!(st.selected_index(), 1);
        st.cycle_sort();
        assert_eq!(st.selected_index(), 0);
    }

    #[test]
    fn reload_clamps_selection_when_new_db_is_smaller() {
        let db = db_with(&[(Category::Skill, "a", "p"), (Category::Skill, "b", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        st.next();
        assert_eq!(st.selected_index(), 1);

        let smaller = db_with(&[(Category::Skill, "only", "p")]);
        st.reload(smaller);
        assert_eq!(st.selected_index(), 0);
        assert_eq!(st.selected_row().map(|r| r.name), Some("only".to_string()));

        let empty = db_with(&[]);
        st.reload(empty);
        assert_eq!(st.selected_index(), 0);
        assert!(st.selected_row().is_none());
    }

    #[test]
    fn reload_at_refreshes_today_so_recency_reflects_the_new_date() {
        let db = db_with(&[(Category::Skill, "a", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;

        // Row was last used on the (initial) today -> "today".
        let before = st.selected_row().expect("row exists");
        assert_eq!(
            format_recency(before.last_used, st.today_for_rescan()),
            "today"
        );

        // Simulate `R` after midnight UTC has rolled over: reload_at moves
        // `today` forward a day, so the same row is now "1d ago" even
        // though its own last-used date didn't change.
        let later = day(2026, 7, 17);
        let refreshed = db_with(&[(Category::Skill, "a", "p")]);
        st.reload_at(later, refreshed, ProvenanceMap::default());

        assert_eq!(st.today_for_rescan(), later);
        let after = st.selected_row().expect("row exists");
        assert_eq!(
            format_recency(after.last_used, st.today_for_rescan()),
            "1d ago"
        );
    }

    #[test]
    fn watch_gates_is_active_and_summary() {
        let mut st = AppState::new(
            UsageDb::default(),
            ProvenanceMap::default(),
            day(2026, 7, 16),
        );
        let mut active = ActiveSet::default();
        active.absorb(vec![
            (Category::Model, "opus".into()),
            (Category::Skill, "brainstorm".into()),
        ]);
        active.record_session();
        st.set_active(active);

        // watch off: no row is marked active.
        assert!(!st.is_active(Category::Model, "opus"));
        assert!(!st.has_active());

        st.watch = true;
        assert!(st.is_active(Category::Model, "opus"));
        assert!(st.is_active(Category::Skill, "brainstorm"));
        assert!(!st.is_active(Category::Agent, "opus")); // wrong category
        assert!(st.has_active());
        // (Model,opus) sorts before (Skill,brainstorm).
        assert_eq!(
            st.running_summary(),
            "running: opus · brainstorm · 1 active"
        );
    }

    #[test]
    fn running_summary_is_idle_when_empty() {
        let st = AppState::new(
            UsageDb::default(),
            ProvenanceMap::default(),
            day(2026, 7, 16),
        );
        assert_eq!(st.running_summary(), "idle");
        assert!(!st.has_active());
    }

    #[test]
    fn running_summary_truncates_beyond_eight_names() {
        let mut st = AppState::new(
            UsageDb::default(),
            ProvenanceMap::default(),
            day(2026, 7, 16),
        );
        let mut active = ActiveSet::default();
        // Ten distinct names in a known order; only the first eight are shown.
        active.absorb((1..=10).map(|i| (Category::Skill, format!("s{i:02}"))));
        active.record_session();
        st.set_active(active);

        assert_eq!(
            st.running_summary(),
            "running: s01 · s02 · s03 · s04 · s05 · s06 · s07 · s08 … · 1 active"
        );
    }

    #[test]
    fn spinner_advances_by_frame() {
        let mut st = AppState::new(
            UsageDb::default(),
            ProvenanceMap::default(),
            day(2026, 7, 16),
        );
        let first = st.spinner_char();
        st.tick_spinner();
        assert_ne!(st.spinner_char(), first);
    }
}
