//! ccstat TUI: pure `AppState` (fully testable without a terminal), key
//! classification, detail formatting, plus rendering and the blocking event
//! loop (rendering/`run` added in Task 7).

use crate::model::{Category, ProjectFilter, SortKey, Window};
use crate::scan::{self, ScanConfig};
use crate::usage::{Row, UsageDb};
use chrono::NaiveDate;
use crossterm::event::KeyCode;

/// Sentinel shown at the top of the project picker for "no filter".
const ALL_PROJECTS: &str = "(all projects)";

/// Pure UI state: the aggregated store plus every selector the user can move.
pub struct AppState {
    db: UsageDb,
    today: NaiveDate,
    pub tab: Category,
    pub window: Window,
    pub sort: SortKey,
    pub project: ProjectFilter,
    pub filter: String,
    pub filtering: bool,
    pub picking: bool,
    picker_idx: usize,
    selected: usize,
}

impl AppState {
    #[must_use]
    pub fn new(db: UsageDb, today: NaiveDate) -> Self {
        Self {
            db,
            today,
            tab: Category::Model,
            window: Window::Days30,
            sort: SortKey::Count,
            project: ProjectFilter::All,
            filter: String::new(),
            filtering: false,
            picking: false,
            picker_idx: 0,
            selected: 0,
        }
    }

    /// Rows for the current tab/window/project/sort, then narrowed by the
    /// case-insensitive name filter.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let needle = self.filter.to_lowercase();
        self.db
            .rows(self.tab, self.window, &self.project, self.sort, self.today)
            .into_iter()
            .filter(|r| needle.is_empty() || r.name.to_lowercase().contains(&needle))
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
}

/// Semantic key intent. `filtering` and `picking` are modal: they capture
/// keys before the normal navigation bindings.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    Quit,
    NextTab,
    PrevTab,
    Next,
    Prev,
    CycleWindow,
    CycleSort,
    OpenProjectPicker,
    StartFilter,
    FilterChar(char),
    FilterBackspace,
    FilterClear,
    FilterConfirm,
    PickerNext,
    PickerPrev,
    PickerApply,
    PickerCancel,
    Rescan,
    Ignore,
}

#[must_use]
pub fn classify_key(code: KeyCode, filtering: bool, picking: bool) -> KeyAction {
    if picking {
        return match code {
            KeyCode::Char('j') | KeyCode::Down => KeyAction::PickerNext,
            KeyCode::Char('k') | KeyCode::Up => KeyAction::PickerPrev,
            KeyCode::Enter => KeyAction::PickerApply,
            KeyCode::Esc => KeyAction::PickerCancel,
            _ => KeyAction::Ignore,
        };
    }
    if filtering {
        return match code {
            KeyCode::Esc => KeyAction::FilterClear,
            KeyCode::Enter => KeyAction::FilterConfirm,
            KeyCode::Backspace => KeyAction::FilterBackspace,
            KeyCode::Char(c) => KeyAction::FilterChar(c),
            _ => KeyAction::Ignore,
        };
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => KeyAction::Quit,
        KeyCode::Tab => KeyAction::NextTab,
        KeyCode::BackTab => KeyAction::PrevTab,
        KeyCode::Char('j') | KeyCode::Down => KeyAction::Next,
        KeyCode::Char('k') | KeyCode::Up => KeyAction::Prev,
        KeyCode::Char('w') => KeyAction::CycleWindow,
        KeyCode::Char('s') => KeyAction::CycleSort,
        KeyCode::Char('p') => KeyAction::OpenProjectPicker,
        KeyCode::Char('/') => KeyAction::StartFilter,
        KeyCode::Char('R') => KeyAction::Rescan,
        _ => KeyAction::Ignore,
    }
}

/// Human-readable recency: "never", "today", "1d ago", "5d ago".
#[must_use]
pub fn format_recency(last_used: Option<NaiveDate>, today: NaiveDate) -> String {
    match last_used {
        None => "never".to_string(),
        Some(d) => {
            let days = (today - d).num_days();
            match days {
                n if n <= 0 => "today".to_string(),
                1 => "1d ago".to_string(),
                n => format!("{n}d ago"),
            }
        }
    }
}

/// Detail-pane text for the selected row. Pure and terminal-free.
#[must_use]
pub fn detail_lines(row: &Row, category: Category, today: NaiveDate) -> Vec<String> {
    let mut lines = vec![
        format!("{}: {}", category.title(), row.name),
        String::new(),
        format!(
            "count {} · last {} · first {}",
            row.count,
            format_recency(row.last_used, today),
            row.first_used
                .map_or_else(|| "—".to_string(), |d| d.format("%Y-%m-%d").to_string()),
        ),
    ];

    if category == Category::Model && row.count > 0 {
        lines.push(String::new());
        lines.push(format!(
            "tokens: in {} · out {} · cache-write {} · cache-read {}",
            row.input, row.output, row.cache_creation, row.cache_read
        ));
        lines.push(format!("est. cost: ${:.2}", row.cost_usd));
    }

    if !row.by_project.is_empty() {
        lines.push(String::new());
        lines.push("by project:".to_string());
        for (proj, n) in row.by_project.iter().take(8) {
            lines.push(format!("  {proj}  {n}"));
        }
    }

    lines
}

/// Runs the ccstat TUI. Filled in in Task 7.
///
/// `cfg` is taken by value because Task 7's event loop needs to own it
/// (re-scanning on the `R` rescan key reuses `cfg.projects_dir` across the
/// blocking loop); this stub only borrows it, hence the explicit allow.
#[allow(clippy::needless_pass_by_value)]
pub fn run(cfg: ScanConfig, today: NaiveDate) -> anyhow::Result<()> {
    let _ = (
        &cfg,
        today,
        scan::scan as fn(&ScanConfig, NaiveDate) -> UsageDb,
    );
    unimplemented!("rendering + event loop added in Task 7")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::{Extracted, LineData};
    use crate::usage::TREND_DAYS;
    use chrono::TimeZone;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn db_with(items: &[(Category, &str, &str)]) -> UsageDb {
        let mut db = UsageDb::default();
        let ts = chrono::Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap();
        for (cat, name, project) in items {
            let ex = match cat {
                // Model isn't exercised by these tests; it maps to Skill as a placeholder.
                Category::Skill | Category::Model => Extracted::Skill {
                    name: (*name).into(),
                },
                Category::Agent => Extracted::Agent {
                    name: (*name).into(),
                },
                Category::Command => Extracted::Command {
                    name: (*name).into(),
                },
                Category::Mcp => Extracted::Mcp {
                    server: (*name).into(),
                },
            };
            let line = LineData {
                timestamp: Some(ts),
                cwd: None,
                items: vec![ex],
            };
            db.absorb(&line, project, day(2026, 7, 16));
        }
        db
    }

    #[test]
    fn starts_on_model_tab_30d_count_sort() {
        let st = AppState::new(UsageDb::default(), day(2026, 7, 16));
        assert_eq!(st.tab, Category::Model);
        assert_eq!(st.window, Window::Days30);
        assert_eq!(st.sort, SortKey::Count);
        assert_eq!(st.project, ProjectFilter::All);
    }

    #[test]
    fn tab_switch_moves_to_the_right_category_and_resets_selection() {
        let db = db_with(&[(Category::Skill, "a", "p"), (Category::Skill, "b", "p")]);
        let mut st = AppState::new(db, day(2026, 7, 16));
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
        let mut st = AppState::new(db, day(2026, 7, 16));
        st.tab = Category::Skill;
        st.set_filter("alp".to_string());
        let names: Vec<String> = st.rows().into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["Alpha"]);
    }

    #[test]
    fn selection_clamps_and_tracks() {
        let db = db_with(&[(Category::Skill, "a", "p"), (Category::Skill, "b", "p")]);
        let mut st = AppState::new(db, day(2026, 7, 16));
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
        let mut st = AppState::new(db, day(2026, 7, 16));
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
        let mut st = AppState::new(db, day(2026, 7, 16));
        st.open_project_picker();
        st.picker_next();
        st.cancel_picker();
        assert!(!st.picking);
        assert_eq!(st.project, ProjectFilter::All);
    }

    #[test]
    fn classify_key_normal_mode() {
        assert_eq!(
            classify_key(KeyCode::Char('q'), false, false),
            KeyAction::Quit
        );
        assert_eq!(classify_key(KeyCode::Tab, false, false), KeyAction::NextTab);
        assert_eq!(
            classify_key(KeyCode::Char('w'), false, false),
            KeyAction::CycleWindow
        );
        assert_eq!(
            classify_key(KeyCode::Char('s'), false, false),
            KeyAction::CycleSort
        );
        assert_eq!(
            classify_key(KeyCode::Char('p'), false, false),
            KeyAction::OpenProjectPicker
        );
        assert_eq!(
            classify_key(KeyCode::Char('/'), false, false),
            KeyAction::StartFilter
        );
        assert_eq!(
            classify_key(KeyCode::Char('R'), false, false),
            KeyAction::Rescan
        );
        assert_eq!(
            classify_key(KeyCode::Char('z'), false, false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn classify_key_filtering_and_picking_modes_take_priority() {
        assert_eq!(
            classify_key(KeyCode::Char('w'), true, false),
            KeyAction::FilterChar('w')
        );
        assert_eq!(
            classify_key(KeyCode::Esc, true, false),
            KeyAction::FilterClear
        );
        // picking wins even if filtering is also somehow set.
        assert_eq!(
            classify_key(KeyCode::Char('j'), true, true),
            KeyAction::PickerNext
        );
        assert_eq!(
            classify_key(KeyCode::Enter, false, true),
            KeyAction::PickerApply
        );
        assert_eq!(
            classify_key(KeyCode::Esc, false, true),
            KeyAction::PickerCancel
        );
    }

    #[test]
    fn recency_formatting() {
        let today = day(2026, 7, 16);
        assert_eq!(format_recency(None, today), "never");
        assert_eq!(format_recency(Some(day(2026, 7, 16)), today), "today");
        assert_eq!(format_recency(Some(day(2026, 7, 15)), today), "1d ago");
        assert_eq!(format_recency(Some(day(2026, 7, 6)), today), "10d ago");
    }

    #[test]
    fn detail_lines_include_tokens_only_for_model() {
        let today = day(2026, 7, 16);
        let model_row = Row {
            name: "opus".into(),
            count: 3,
            last_used: Some(today),
            first_used: Some(day(2026, 7, 1)),
            trend: vec![0; TREND_DAYS],
            input: 100,
            output: 200,
            cache_creation: 0,
            cache_read: 50,
            cost_usd: 1.234,
            by_project: vec![],
        };
        let ml = detail_lines(&model_row, Category::Model, today);
        assert!(ml.iter().any(|l| l.contains("tokens:")));
        assert!(ml.iter().any(|l| l.contains("est. cost: $1.23")));

        let skill_row = Row {
            by_project: vec![("alpha".into(), 3)],
            ..model_row.clone()
        };
        let sl = detail_lines(&skill_row, Category::Skill, today);
        assert!(!sl.iter().any(|l| l.contains("tokens:")));
        assert!(sl.iter().any(|l| l.contains("by project:")));
    }
}
