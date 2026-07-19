//! ccstat TUI: pure `AppState` (fully testable without a terminal), key
//! classification, detail formatting, rendering, and the two event loops —
//! the blocking snapshot loop and the `--watch` live loop that animates the
//! running-item spinner and auto-refreshes.

use crate::live::{self, ActiveSet};
use crate::model::{Category, ProjectFilter, SortKey, Window};
use crate::provenance::ProvenanceMap;
use crate::scan::{self, ScanConfig};
use crate::usage::{Row, TREND_DAYS, UsageDb};
use ccmap::model::Provenance;
use cctk::pricing::ModelInfo;
use chrono::{Duration, NaiveDate};
use crossterm::event::{Event as CtEvent, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};

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
    ToggleGraph,
    Ignore,
}

#[must_use]
pub fn classify_key(
    code: KeyCode,
    filtering: bool,
    picking: bool,
    showing_graph: bool,
) -> KeyAction {
    if showing_graph {
        return match code {
            KeyCode::Char('g') | KeyCode::Esc => KeyAction::ToggleGraph,
            KeyCode::Char('q') => KeyAction::Quit,
            _ => KeyAction::Ignore,
        };
    }
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
        KeyCode::Char('g') => KeyAction::ToggleGraph,
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
const RUN_H: u16 = 1;
const HINT_H: u16 = 1;
const LIST_PCT: u16 = 45;
const DETAIL_PCT: u16 = 55;

/// Spinner animation frame interval (and the max time an idle watch loop
/// blocks on input before redrawing).
const SPINNER_TICK: std::time::Duration = std::time::Duration::from_millis(100);
/// How often watch mode recomputes the "running now" set (cheap tail reads).
const ACTIVE_POLL: std::time::Duration = std::time::Duration::from_secs(2);
/// A session counts as live if its log was modified within this many seconds.
const ACTIVE_WINDOW_SECS: i64 = 90;
/// Bytes read from the end of each live session log for activity detection.
const TAIL_BYTES: u64 = 16 * 1024;

/// Runs the ccstat TUI to completion: scans logs, discovers extension
/// provenance, enters the alternate screen + raw mode, and drives the event
/// loop until the user quits. `watch` selects the loop: `None` is the classic
/// blocking snapshot loop; `Some(interval)` is the live loop that animates the
/// spinner, polls activity, and re-aggregates every `interval`. The terminal
/// is always restored, on success and error alike.
pub fn run(
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
    today: NaiveDate,
    watch: Option<std::time::Duration>,
) -> anyhow::Result<()> {
    let db = scan::scan(cfg, today);
    let provenance = ProvenanceMap::build(&ccmap::discover::discover_extensions(ctx));
    let mut state = AppState::new(db, provenance, today);
    state.watch = watch.is_some();
    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let result = match watch {
        Some(interval) => run_watch_inner(&mut terminal, cfg, ctx, &mut state, interval),
        None => run_inner(&mut terminal, cfg, ctx, &mut state),
    };
    ratatui::restore();
    result
}

/// Live event loop for `--watch`. Uses `event::poll` so the loop wakes on the
/// spinner timer even without input: each timeout advances the animation,
/// `ACTIVE_POLL` recomputes the running set from live-session tails, and
/// `rescan_interval` re-aggregates the whole corpus (like pressing `R`).
fn run_watch_inner(
    terminal: &mut DefaultTerminal,
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
    state: &mut AppState,
    rescan_interval: std::time::Duration,
) -> anyhow::Result<()> {
    let window = Duration::seconds(ACTIVE_WINDOW_SECS);
    // Seed the running set so the very first frame reflects live activity.
    state.set_active(scan::compute_active(
        cfg,
        chrono::Utc::now(),
        window,
        TAIL_BYTES,
    ));
    let mut last_active = std::time::Instant::now();
    let mut last_rescan = std::time::Instant::now();

    loop {
        terminal.draw(|f| draw(f, state))?;

        if crossterm::event::poll(SPINNER_TICK)? {
            if let CtEvent::Key(key) = crossterm::event::read()?
                && key.kind == KeyEventKind::Press
                && handle_key(key.code, state, cfg, ctx)
            {
                return Ok(());
            }
        } else {
            // Poll timed out with no input: advance the spinner animation.
            state.tick_spinner();
        }

        let now = std::time::Instant::now();
        if now.duration_since(last_active) >= ACTIVE_POLL {
            state.set_active(scan::compute_active(
                cfg,
                chrono::Utc::now(),
                window,
                TAIL_BYTES,
            ));
            last_active = now;
        }
        if now.duration_since(last_rescan) >= rescan_interval {
            let today = chrono::Utc::now().date_naive();
            let provenance = ProvenanceMap::build(&ccmap::discover::discover_extensions(ctx));
            state.reload_at(today, scan::scan(cfg, today), provenance);
            last_rescan = now;
        }
    }
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
///
/// Part of the re-entrant view API: a host (e.g. `cctop` drill-down) can own
/// the terminal and event loop, calling [`draw`] each frame and `handle_key`
/// per keypress. A `true` return means "leave this view" (back to the host).
pub fn handle_key(
    code: KeyCode,
    state: &mut AppState,
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
) -> bool {
    match classify_key(code, state.filtering, state.picking, state.showing_graph) {
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
        KeyAction::ToggleGraph => state.toggle_graph(),
        KeyAction::Ignore => {}
    }
    false
}

/// Render the full ccstat view into `f`. Public so a host (e.g. `cctop`
/// drill-down) can render this view without ccstat owning the terminal.
pub fn draw(f: &mut Frame<'_>, state: &AppState) {
    // Watch mode inserts a one-line running summary between the tabs and body.
    let mut constraints = vec![Constraint::Length(TABS_H)];
    if state.watch {
        constraints.push(Constraint::Length(RUN_H));
    }
    constraints.push(Constraint::Min(0));
    constraints.push(Constraint::Length(HINT_H));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.area());

    let mut idx = 0;
    draw_tabs(f, chunks[idx], state);
    idx += 1;
    if state.watch {
        draw_running(f, chunks[idx], state);
        idx += 1;
    }
    let body = chunks[idx];
    idx += 1;
    let hint = chunks[idx];

    if state.showing_graph && state.selected_row().is_some() {
        draw_graph(f, body, state);
    } else {
        draw_body(f, body, state);
    }
    draw_hint(f, hint, state);

    if state.picking {
        draw_project_picker(f, f.area(), state);
    }
}

/// One-line running summary shown in watch mode: an animated spinner (or a dim
/// `○` when idle) followed by the running-item names and live-session count.
fn draw_running(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let (glyph, glyph_style) = if state.has_active() {
        (
            state.spinner_char(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ('○', Style::default().fg(Color::DarkGray))
    };
    let line = Line::from(vec![
        Span::styled(format!(" {glyph} "), glyph_style),
        Span::styled(state.running_summary(), Style::default().fg(Color::Gray)),
    ]);
    f.render_widget(Paragraph::new(line), area);
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
    let watch = if state.watch { " · watch" } else { "" };
    let title = format!(
        " ccstat · window: {} · project: {} · sort: {}{watch} ",
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

/// Full-screen bottom-style braille line chart of the selected row's
/// `TREND_DAYS`-day trend.
fn draw_graph(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let Some(row) = state.selected_row() else {
        return;
    };
    let today = state.today_for_rescan();

    #[allow(clippy::cast_possible_wrap)] // TREND_DAYS is a small constant
    let oldest_offset = (TREND_DAYS - 1) as i64;
    let oldest = (today - Duration::days(oldest_offset))
        .format("%m/%d")
        .to_string();
    let newest = today.format("%m/%d").to_string();

    let points: Vec<(f64, f64)> = row
        .trend
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v as f64))
        .collect();
    let peak = row.trend.iter().copied().max().unwrap_or(0).max(1);
    let last_x = (TREND_DAYS.saturating_sub(1)).max(1) as f64;

    let title = format!(
        " {}: {} · {} in {} · {}-day trend · last {} (g/Esc close) ",
        state.tab.title(),
        row.name,
        row.count,
        state.window.label(),
        TREND_DAYS,
        format_recency(row.last_used, today),
    );

    let chart = cctk::chart::braille_line(&points, peak as f64, peak.to_string(), Color::Green)
        .block(Block::default().borders(Borders::ALL).title(title))
        .x_axis(
            Axis::default()
                .bounds([0.0, last_x])
                .labels([Line::from(oldest), Line::from(newest)])
                .style(Style::default().fg(Color::DarkGray)),
        );
    f.render_widget(chart, area);
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
            // In watch mode a leading column carries the spinner for rows that
            // are running now; idle rows (and all rows in snapshot mode) keep
            // the classic layout.
            if state.watch {
                let spin = if state.is_active(state.tab, &r.name) {
                    state.spinner_char()
                } else {
                    ' '
                };
                let spin_span = Span::styled(
                    spin.to_string(),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                );
                let rest = Span::styled(
                    format!(" {:>6}  {}  {}", r.count, bar, r.name),
                    Style::default().fg(color),
                );
                ListItem::new(Line::from(vec![spin_span, rest]))
            } else {
                let label = format!("{:>6}  {}  {}", r.count, bar, r.name);
                ListItem::new(Line::from(Span::styled(label, Style::default().fg(color))))
            }
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
    let text = if state.showing_graph {
        " g/Esc close graph · q quit "
    } else if state.picking {
        " j/k move · Enter select · Esc cancel "
    } else if state.filtering {
        " type to filter · Enter confirm · Esc clear "
    } else {
        " q quit · Tab tabs · j/k move · w window · s sort · p project · g graph · / filter · R rescan "
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

/// A fixed-width braille dot bar scaled to `count / max`.
fn count_bar(count: u64, max: u64) -> String {
    const WIDTH: usize = 10;
    let ratio = if max == 0 {
        0.0
    } else {
        count as f64 / max as f64
    };
    cctk::viz::dot_bar(ratio, WIDTH)
}

/// A braille dot sparkline over daily counts (one cell per value).
fn sparkline(values: &[u64]) -> String {
    let floats: Vec<f64> = values.iter().map(|&v| v as f64).collect();
    cctk::viz::sparkline(&floats, values.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ProvenanceMap;
    use crate::usage::LineData;
    use crate::usage::TREND_DAYS;
    use cctk::jsonl::Extracted;
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
            classify_key(KeyCode::Char('q'), false, false, false),
            KeyAction::Quit
        );
        assert_eq!(
            classify_key(KeyCode::Tab, false, false, false),
            KeyAction::NextTab
        );
        assert_eq!(
            classify_key(KeyCode::Char('w'), false, false, false),
            KeyAction::CycleWindow
        );
        assert_eq!(
            classify_key(KeyCode::Char('s'), false, false, false),
            KeyAction::CycleSort
        );
        assert_eq!(
            classify_key(KeyCode::Char('p'), false, false, false),
            KeyAction::OpenProjectPicker
        );
        assert_eq!(
            classify_key(KeyCode::Char('/'), false, false, false),
            KeyAction::StartFilter
        );
        assert_eq!(
            classify_key(KeyCode::Char('R'), false, false, false),
            KeyAction::Rescan
        );
        assert_eq!(
            classify_key(KeyCode::Char('z'), false, false, false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn classify_key_filtering_and_picking_modes_take_priority() {
        assert_eq!(
            classify_key(KeyCode::Char('w'), true, false, false),
            KeyAction::FilterChar('w')
        );
        assert_eq!(
            classify_key(KeyCode::Esc, true, false, false),
            KeyAction::FilterClear
        );
        // picking wins even if filtering is also somehow set.
        assert_eq!(
            classify_key(KeyCode::Char('j'), true, true, false),
            KeyAction::PickerNext
        );
        assert_eq!(
            classify_key(KeyCode::Enter, false, true, false),
            KeyAction::PickerApply
        );
        assert_eq!(
            classify_key(KeyCode::Esc, false, true, false),
            KeyAction::PickerCancel
        );
    }

    #[test]
    fn classify_key_graph_mode_is_outermost_modal() {
        // In graph mode, g/Esc close; q quits; navigation is ignored.
        assert_eq!(
            classify_key(KeyCode::Char('g'), false, false, true),
            KeyAction::ToggleGraph
        );
        assert_eq!(
            classify_key(KeyCode::Esc, false, false, true),
            KeyAction::ToggleGraph
        );
        assert_eq!(
            classify_key(KeyCode::Char('q'), false, false, true),
            KeyAction::Quit
        );
        assert_eq!(
            classify_key(KeyCode::Char('j'), false, false, true),
            KeyAction::Ignore
        );
        // Graph beats filtering/picking flags if somehow both set.
        assert_eq!(
            classify_key(KeyCode::Char('g'), true, true, true),
            KeyAction::ToggleGraph
        );
    }

    #[test]
    fn classify_key_g_opens_graph_in_normal_mode() {
        assert_eq!(
            classify_key(KeyCode::Char('g'), false, false, false),
            KeyAction::ToggleGraph
        );
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
        // 10 cells = 20 braille columns.
        assert_eq!(super::count_bar(0, 10), "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀");
        assert_eq!(super::count_bar(10, 10), "⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿");
        assert_eq!(super::count_bar(5, 10), "⣿⣿⣿⣿⣿⠀⠀⠀⠀⠀");
    }

    #[test]
    fn count_bar_zero_max_is_blank() {
        assert_eq!(super::count_bar(5, 0), "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀");
    }

    #[test]
    fn sparkline_marks_zero_days_as_blank_and_scales_up() {
        assert_eq!(super::sparkline(&[0, 0, 0]), "⠀⠀⠀");
        let s = super::sparkline(&[0, 1, 8]);
        assert_eq!(s.chars().count(), 3);
        assert!(s.starts_with('⠀'));
        assert_ne!(s.chars().last().unwrap(), '⠀');
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

    #[test]
    fn watch_mode_renders_running_line_and_active_spinner() {
        use ratatui::{Terminal, backend::TestBackend};

        let db = db_with(&[
            (Category::Skill, "brainstorm", "p"),
            (Category::Skill, "idle-one", "p"),
        ]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        st.watch = true;
        let mut active = ActiveSet::default();
        active.absorb(vec![(Category::Skill, "brainstorm".into())]);
        active.record_session();
        st.set_active(active);

        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|f| draw(f, &st)).unwrap();
        let grid: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();

        // The running-summary line is present.
        assert!(
            grid.contains("running: brainstorm · 1 active"),
            "grid: {grid}"
        );
        // Some spinner frame is drawn (the active row / header glyph).
        let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        assert!(frames.iter().any(|g| grid.contains(*g)), "no spinner frame");
    }

    #[test]
    fn snapshot_mode_has_no_running_line() {
        use ratatui::{Terminal, backend::TestBackend};

        let db = db_with(&[(Category::Skill, "brainstorm", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        // watch stays false.

        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|f| draw(f, &st)).unwrap();
        let grid: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();

        assert!(!grid.contains("running:"));
        assert!(!grid.contains("· watch"));
    }
}
