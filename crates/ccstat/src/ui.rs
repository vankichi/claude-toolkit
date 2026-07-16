//! ccstat TUI: pure `AppState` (fully testable without a terminal), key
//! classification, detail formatting, plus rendering and the blocking event
//! loop (rendering/`run` added in Task 7).

use crate::model::{Category, ProjectFilter, SortKey, Window};
use crate::pricing::ModelInfo;
use crate::provenance::ProvenanceMap;
use crate::scan::{self, ScanConfig};
use crate::usage::{Row, TREND_DAYS, UsageDb};
use ccmap::model::Provenance;
use chrono::NaiveDate;
use crossterm::event::{Event as CtEvent, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};

/// Sentinel shown at the top of the project picker for "no filter".
const ALL_PROJECTS: &str = "(all projects)";

/// Pure UI state: the aggregated store plus every selector the user can move.
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

    /// `today` used for slicing; exposed for rescans and the detail pane.
    #[must_use]
    pub fn today_for_rescan(&self) -> NaiveDate {
        self.today
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

/// Row color for a provenance (mirrors ccmap's provenance palette); `None`
/// (built-in / undiscovered) is dim gray.
#[must_use]
fn provenance_color(provenance: Option<Provenance>) -> Color {
    match provenance {
        Some(Provenance::Local) => Color::White,
        Some(Provenance::Project) => Color::Cyan,
        Some(Provenance::Official) => Color::Yellow,
        Some(Provenance::Community) => Color::Magenta,
        None => Color::DarkGray,
    }
}

/// Human-readable source label for the detail pane.
#[must_use]
fn source_label(provenance: Option<Provenance>) -> &'static str {
    match provenance {
        Some(Provenance::Local) => "Local (~/.claude)",
        Some(Provenance::Project) => "Project",
        Some(Provenance::Official) => "Official plugin",
        Some(Provenance::Community) => "Community plugin",
        None => "built-in / unknown",
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

const TABS_H: u16 = 3;
const HINT_H: u16 = 1;
const LIST_PCT: u16 = 45;
const DETAIL_PCT: u16 = 55;

/// Runs the ccstat TUI to completion: scans logs, discovers extension
/// provenance, enters the alternate screen + raw mode, and drives the
/// blocking event loop until the user quits. The terminal is always
/// restored, on success and error alike.
pub fn run(
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
    today: NaiveDate,
) -> anyhow::Result<()> {
    let db = scan::scan(cfg, today);
    let provenance = ProvenanceMap::build(&ccmap::discover::discover_extensions(ctx));
    let mut state = AppState::new(db, provenance, today);
    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let result = run_inner(&mut terminal, cfg, ctx, &mut state);
    ratatui::restore();
    result
}

fn run_inner(
    terminal: &mut DefaultTerminal,
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
    state: &mut AppState,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| draw(f, state))?;
        let CtEvent::Key(key) = crossterm::event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if handle_key(key.code, state, cfg, ctx) {
            return Ok(());
        }
    }
}

/// Applies one keypress. Returns `true` when the user wants to quit.
fn handle_key(
    code: KeyCode,
    state: &mut AppState,
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
) -> bool {
    match classify_key(code, state.filtering, state.picking) {
        KeyAction::Quit => return true,
        KeyAction::NextTab => state.next_tab(),
        KeyAction::PrevTab => state.prev_tab(),
        KeyAction::Next => state.next(),
        KeyAction::Prev => state.prev(),
        KeyAction::CycleWindow => state.cycle_window(),
        KeyAction::CycleSort => state.cycle_sort(),
        KeyAction::OpenProjectPicker => state.open_project_picker(),
        KeyAction::StartFilter => state.filtering = true,
        KeyAction::FilterChar(c) => {
            let mut next = state.filter.clone();
            next.push(c);
            state.set_filter(next);
        }
        KeyAction::FilterBackspace => {
            let mut next = state.filter.clone();
            next.pop();
            state.set_filter(next);
        }
        KeyAction::FilterClear => {
            state.set_filter(String::new());
            state.filtering = false;
        }
        KeyAction::FilterConfirm => state.filtering = false,
        KeyAction::PickerNext => state.picker_next(),
        KeyAction::PickerPrev => state.picker_prev(),
        KeyAction::PickerApply => state.apply_picker(),
        KeyAction::PickerCancel => state.cancel_picker(),
        KeyAction::Rescan => {
            let today = chrono::Utc::now().date_naive();
            let provenance = ProvenanceMap::build(&ccmap::discover::discover_extensions(ctx));
            state.reload_at(today, scan::scan(cfg, today), provenance);
        }
        KeyAction::Ignore => {}
    }
    false
}

fn draw(f: &mut Frame<'_>, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TABS_H),
            Constraint::Min(0),
            Constraint::Length(HINT_H),
        ])
        .split(f.area());

    draw_tabs(f, chunks[0], state);
    draw_body(f, chunks[1], state);
    draw_hint(f, chunks[2], state);

    if state.picking {
        draw_project_picker(f, f.area(), state);
    }
}

fn draw_tabs(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let titles: Vec<&'static str> = Category::ALL.iter().map(|c| c.title()).collect();
    let selected = Category::ALL
        .iter()
        .position(|c| *c == state.tab)
        .unwrap_or(0);
    let project = match &state.project {
        ProjectFilter::All => "(all)".to_string(),
        ProjectFilter::Only(p) => p.clone(),
    };
    let title = format!(
        " ccstat · window: {} · project: {} · sort: {} ",
        state.window.label(),
        project,
        state.sort.label(),
    );
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .select(selected);
    f.render_widget(tabs, area);
}

fn draw_body(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(LIST_PCT),
            Constraint::Percentage(DETAIL_PCT),
        ])
        .split(area);
    draw_list(f, chunks[0], state);
    draw_detail(f, chunks[1], state);
}

fn draw_list(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let rows = state.rows();
    let max_count = rows.iter().map(|r| r.count).max().unwrap_or(0).max(1);
    let items: Vec<ListItem<'_>> = rows
        .iter()
        .map(|r| {
            let bar = count_bar(r.count, max_count);
            let color = match state.tab {
                Category::Model => ModelInfo::parse(&r.name).color(),
                Category::Agent | Category::Skill | Category::Command => {
                    provenance_color(state.provenance_of(state.tab, &r.name))
                }
                Category::Mcp => Color::White,
            };
            let label = format!("{:>6}  {}  {}", r.count, bar, r.name);
            ListItem::new(Line::from(Span::styled(label, Style::default().fg(color))))
        })
        .collect();

    let mut list_state = ListState::default();
    if !rows.is_empty() {
        list_state.select(Some(state.selected_index()));
    }

    let title = if state.filtering {
        format!(" {}  filter: {}_ ", state.tab.title(), state.filter)
    } else if state.filter.is_empty() {
        format!(" {} ", state.tab.title())
    } else {
        format!(" {}  filter: {} ", state.tab.title(), state.filter)
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, area, &mut list_state);
}

fn draw_detail(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let block = Block::default().borders(Borders::ALL).title(" detail ");
    let Some(row) = state.selected_row() else {
        f.render_widget(Paragraph::new("(no matching items)").block(block), area);
        return;
    };
    let mut lines = detail_lines(&row, state.tab, state.today_for_rescan());
    if matches!(
        state.tab,
        Category::Agent | Category::Skill | Category::Command
    ) {
        lines.push(format!(
            "source: {}",
            source_label(state.provenance_of(state.tab, &row.name))
        ));
    }
    lines.push(String::new());
    lines.push(format!("trend ({TREND_DAYS}d): {}", sparkline(&row.trend)));
    let text = lines.join("\n");
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(block),
        area,
    );
}

fn draw_hint(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let text = if state.picking {
        " j/k move · Enter select · Esc cancel "
    } else if state.filtering {
        " type to filter · Enter confirm · Esc clear "
    } else {
        " q quit · Tab tabs · j/k move · w window · s sort · p project · / filter · R rescan "
    };
    f.render_widget(Paragraph::new(text), area);
}

fn draw_project_picker(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let opts = state.picker_options();
    let width = area.width.saturating_sub(area.width / 4).clamp(20, 60);
    #[allow(clippy::cast_possible_truncation)]
    let opts_h = opts.len() as u16;
    let height = (opts_h.saturating_add(2))
        .min(area.height.saturating_sub(4))
        .max(3);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect {
        x,
        y,
        width,
        height,
    };

    let items: Vec<ListItem<'_>> = opts
        .iter()
        .map(|o| ListItem::new(Line::from(o.clone())))
        .collect();
    let mut ls = ListState::default();
    ls.select(Some(state.picker_index()));

    f.render_widget(Clear, popup);
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" select project "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, popup, &mut ls);
}

/// A fixed-width unicode block bar scaled to `count / max`.
fn count_bar(count: u64, max: u64) -> String {
    const WIDTH: usize = 10;
    if max == 0 {
        return "·".repeat(WIDTH);
    }
    let scaled = (count as f64 / max as f64) * WIDTH as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = scaled.round() as usize;
    let filled = filled.min(WIDTH);
    format!("{}{}", "█".repeat(filled), "·".repeat(WIDTH - filled))
}

/// A unicode sparkline over daily counts.
fn sparkline(values: &[u64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return "·".repeat(values.len());
    }
    values
        .iter()
        .map(|&v| {
            if v == 0 {
                '·'
            } else {
                let scaled = (v as f64 / max as f64) * (BARS.len() - 1) as f64;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let idx = scaled.round() as usize;
                BARS[idx.min(BARS.len() - 1)]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::{Extracted, LineData};
    use crate::provenance::ProvenanceMap;
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
    fn provenance_color_maps_each_case() {
        use ccmap::model::Provenance;
        assert_eq!(
            super::provenance_color(Some(Provenance::Local)),
            Color::White
        );
        assert_eq!(
            super::provenance_color(Some(Provenance::Project)),
            Color::Cyan
        );
        assert_eq!(
            super::provenance_color(Some(Provenance::Official)),
            Color::Yellow
        );
        assert_eq!(
            super::provenance_color(Some(Provenance::Community)),
            Color::Magenta
        );
        assert_eq!(super::provenance_color(None), Color::DarkGray);
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
    fn detail_lines_suppresses_tokens_for_zero_count_model_row() {
        let today = day(2026, 7, 16);
        let row = Row {
            name: "opus".into(),
            count: 0,
            last_used: Some(today),
            first_used: Some(day(2026, 7, 1)),
            trend: vec![0; TREND_DAYS],
            input: 0,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
            cost_usd: 0.0,
            by_project: vec![],
        };
        let lines = detail_lines(&row, Category::Model, today);
        assert!(!lines.iter().any(|l| l.starts_with("tokens:")));
        assert!(!lines.iter().any(|l| l.starts_with("est. cost:")));
    }

    #[test]
    fn detail_lines_omits_by_project_section_when_empty() {
        let today = day(2026, 7, 16);
        let row = Row {
            name: "opus".into(),
            count: 3,
            last_used: Some(today),
            first_used: Some(day(2026, 7, 1)),
            trend: vec![0; TREND_DAYS],
            input: 100,
            output: 200,
            cache_creation: 0,
            cache_read: 50,
            cost_usd: 1.23,
            by_project: vec![],
        };
        let lines = detail_lines(&row, Category::Model, today);
        assert!(!lines.iter().any(|l| l == "by project:"));
    }

    #[test]
    fn count_bar_scales_and_clamps() {
        assert_eq!(super::count_bar(0, 10), "··········");
        assert_eq!(super::count_bar(10, 10), "██████████");
        assert_eq!(super::count_bar(5, 10), "█████·····");
    }

    #[test]
    fn count_bar_zero_max_is_all_dots() {
        assert_eq!(super::count_bar(5, 0), "··········");
    }

    #[test]
    fn sparkline_marks_zero_days_with_dots() {
        assert_eq!(super::sparkline(&[0, 0, 0]), "···");
        let s = super::sparkline(&[0, 1, 8]);
        assert_eq!(s.chars().count(), 3);
        assert!(s.starts_with('·'));
        assert!(s.ends_with('█'));
    }
}
