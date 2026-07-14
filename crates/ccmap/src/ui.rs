//! `ccmap`'s TUI: application state, rendering, and the blocking event loop.
//!
//! [`AppState`] is pure, testable data — which category tab is active, the
//! current filter text, and which row is selected — with no terminal or
//! rendering dependencies. [`run`] owns the terminal lifecycle (alternate
//! screen + raw mode, guaranteed restoration on every exit path) and drives
//! a synchronous `crossterm::event::read()` loop on top of it: no tokio, no
//! async, matching `ccmap`'s single-shot CLI usage.

use crate::discover::{self, discover_all};
use crate::model::{Item, Kind, PluginState, Source};
use crossterm::event::{Event as CtEvent, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};

/// The five category tabs, in the fixed order they cycle through.
const TABS: [Kind; 5] = [
    Kind::Agent,
    Kind::Skill,
    Kind::Command,
    Kind::Plugin,
    Kind::Mcp,
];

/// Height of the top tab bar, in rows (including its border).
const TABS_H: u16 = 3;
/// Height of the bottom key-hint line.
const HINT_H: u16 = 1;
/// Width of the left item list as a percentage of the body area.
const LIST_PCT: u16 = 35;
/// Width of the right detail pane as a percentage of the body area.
const DETAIL_PCT: u16 = 65;

/// Configuration for [`run`]: the filesystem roots that discovery scans.
pub struct UiConfig {
    pub ctx: discover::Context,
}

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

    /// The current selection index within `visible()`'s list. Rendering-only
    /// accessor; mutate selection via `next`/`prev`/`set_filter`/`reload`
    /// instead of writing this field directly.
    #[must_use]
    pub fn selected_index(&self) -> usize {
        self.selected
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

/// Runs the `ccmap` TUI to completion: discovers items from `cfg.ctx`,
/// enters the terminal's alternate screen + raw mode, and drives the
/// blocking event loop until the user quits. The terminal is always
/// restored before returning, on both the success and the error path.
pub fn run(cfg: UiConfig) -> anyhow::Result<()> {
    let UiConfig { ctx } = cfg;
    let mut state = AppState::new(discover_all(&ctx).items);
    let mut terminal = ratatui::try_init().inspect_err(|_| restore_terminal())?;
    let result = run_inner(&mut terminal, &ctx, &mut state);
    restore_terminal();
    result
}

/// Restores the terminal to its original mode (raw mode disabled, back to
/// the main screen buffer). Called unconditionally after the event loop
/// returns — success or error — and again around the `e` editor shell-out,
/// so a crash or an early `?` never leaves the caller's shell in
/// raw/alternate-screen mode.
fn restore_terminal() {
    ratatui::restore();
}

/// The blocking event loop: draw a frame, block on `crossterm::event::read()`,
/// classify and dispatch the resulting key press. Returns once the user
/// quits (`q` / `Esc` outside filtering mode).
fn run_inner(
    terminal: &mut DefaultTerminal,
    ctx: &discover::Context,
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
        if handle_key(key.code, state, ctx, terminal)? {
            return Ok(());
        }
    }
}

/// Decoded keyboard intent. Pure mapping from `KeyCode` (+ whether the
/// filter box is being edited) → semantic action, extracted so it can be
/// unit-tested without a terminal.
#[derive(Debug, PartialEq, Eq)]
enum KeyAction {
    Quit,
    NextTab,
    PrevTab,
    Next,
    Prev,
    StartFilter,
    FilterChar(char),
    FilterBackspace,
    FilterClear,
    FilterConfirm,
    OpenEditor,
    CopyClipboard,
    Rescan,
    Ignore,
}

/// Maps a keypress to a semantic action. While `filtering` is active, every
/// key edits the filter text (`Esc` clears it and exits, `Enter` keeps it
/// and exits); otherwise keys drive tab/selection navigation and actions.
fn classify_key(code: KeyCode, filtering: bool) -> KeyAction {
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
        KeyCode::Char('/') => KeyAction::StartFilter,
        KeyCode::Char('e') => KeyAction::OpenEditor,
        KeyCode::Char('y') => KeyAction::CopyClipboard,
        KeyCode::Char('R') => KeyAction::Rescan,
        _ => KeyAction::Ignore,
    }
}

/// Applies a single classified key action to `state`, shelling out for the
/// editor/clipboard actions and rescanning via `ctx` when requested. All
/// list/tab/filter mutation goes through `AppState`'s safe methods; only the
/// `filtering` UI-mode flag is toggled directly. Returns `true` when the
/// caller should exit the event loop.
fn handle_key(
    code: KeyCode,
    state: &mut AppState,
    ctx: &discover::Context,
    terminal: &mut DefaultTerminal,
) -> anyhow::Result<bool> {
    match classify_key(code, state.filtering) {
        KeyAction::Quit => return Ok(true),
        KeyAction::NextTab => state.next_tab(),
        KeyAction::PrevTab => state.prev_tab(),
        KeyAction::Next => state.next(),
        KeyAction::Prev => state.prev(),
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
        KeyAction::OpenEditor => open_editor(state, terminal)?,
        KeyAction::CopyClipboard => {
            if let Some(item) = state.selected_item() {
                copy_to_clipboard(&item.invocation_string());
            }
        }
        KeyAction::Rescan => state.reload(discover_all(ctx).items),
        KeyAction::Ignore => {}
    }
    Ok(false)
}

/// Opens the selected item's source file in `$EDITOR` (falling back to
/// `vi`), suspending the TUI for the duration and restoring it afterwards.
/// No-op if the selected item has no backing file. Mirrors
/// `ccwatch::ui::open_in_browser`'s best-effort philosophy: a failing or
/// missing editor should not tear down the whole TUI, so its exit status is
/// intentionally not propagated.
fn open_editor(state: &AppState, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
    let Some(path) = state.selected_item().and_then(|item| item.path.clone()) else {
        return Ok(());
    };

    restore_terminal();
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(editor).arg(&path).status();
    *terminal = ratatui::try_init()?;
    terminal.clear()?;
    let _ = status;
    Ok(())
}

/// Copies `text` to the system clipboard by piping it over stdin to a
/// platform clipboard tool (never as a CLI argument, so payloads containing
/// shell-sensitive characters are safe). Silently does nothing on
/// unsupported platforms or if the clipboard tool isn't installed.
fn copy_to_clipboard(text: &str) {
    #[cfg(target_os = "macos")]
    let cmd: (&str, &[&str]) = ("pbcopy", &[][..]);
    #[cfg(target_os = "linux")]
    let cmd: (&str, &[&str]) = ("xclip", &["-selection", "clipboard"][..]);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = text;
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        if let Ok(mut child) = Command::new(cmd.0)
            .args(cmd.1)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

/// Renders one frame: tab bar on top, item list + detail pane in the
/// middle, key hints on the bottom.
fn draw(f: &mut Frame<'_>, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TABS_H),
            Constraint::Min(0),
            Constraint::Length(HINT_H),
        ])
        .split(f.area());

    draw_tabs(f, chunks[0], state.tab);
    draw_body(f, chunks[1], state);
    draw_hint(f, chunks[2], state);
}

fn draw_tabs(f: &mut Frame<'_>, area: Rect, active: Kind) {
    let titles: Vec<&'static str> = TABS.iter().copied().map(tab_title).collect();
    let selected = TABS.iter().position(|&kind| kind == active).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(" ccmap "))
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
    let visible = state.visible();
    let items: Vec<ListItem<'_>> = visible
        .iter()
        .map(|item| ListItem::new(item.name.as_str()))
        .collect();

    let mut list_state = ListState::default();
    if !visible.is_empty() {
        list_state.select(Some(state.selected_index()));
    }

    let title = if state.filtering {
        format!(" {}  filter: {}_ ", tab_title(state.tab), state.filter)
    } else if state.filter.is_empty() {
        format!(" {} ", tab_title(state.tab))
    } else {
        format!(" {}  filter: {} ", tab_title(state.tab), state.filter)
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut list_state);
}

fn draw_detail(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let text = state.selected_item().map_or_else(
        || "(no matching items)".to_string(),
        |item| detail_lines(item).join("\n"),
    );
    let paragraph = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(" detail "));
    f.render_widget(paragraph, area);
}

fn draw_hint(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let text = if state.filtering {
        " type to filter · Enter confirm · Esc clear "
    } else {
        " q quit · Tab/⇧Tab switch tab · j/k move · / filter · e edit · y copy · R rescan "
    };
    f.render_widget(Paragraph::new(text), area);
}

/// Display name for a tab's `Kind`, in the fixed order it appears in the
/// tab bar.
#[must_use]
fn tab_title(kind: Kind) -> &'static str {
    match kind {
        Kind::Agent => "Agents",
        Kind::Skill => "Skills",
        Kind::Command => "Commands",
        Kind::Plugin => "Plugins",
        Kind::Mcp => "MCP",
    }
}

/// Human-readable rendering of an item's `Source` for the detail pane.
#[must_use]
fn format_source(source: &Source) -> String {
    match source {
        Source::User => "user".to_string(),
        Source::Project => "project".to_string(),
        Source::Plugin {
            plugin,
            marketplace,
        } => format!("plugin ({plugin}@{marketplace})"),
    }
}

/// Human-readable rendering of a `PluginState` badge for the detail pane.
#[must_use]
fn format_plugin_state(state: PluginState) -> &'static str {
    match state {
        PluginState::Available => "available",
        PluginState::Installed => "installed",
        PluginState::Enabled => "enabled",
    }
}

/// Builds the detail-pane text for `item`: full description, a blank line,
/// `Source: ...`, `Path: ...` when the item has a backing file, a
/// `State: ...` badge for plugins that carry a `PluginState`, and — for
/// agents/commands only — a blank line followed by one `key: value` row per
/// `extra` entry (tools/allowed-tools/model). Pure and terminal-free so it
/// can be unit-tested directly.
#[must_use]
fn detail_lines(item: &Item) -> Vec<String> {
    let mut lines = vec![
        item.description.clone(),
        String::new(),
        format!("Source: {}", format_source(&item.source)),
    ];

    if let Some(path) = &item.path {
        lines.push(format!("Path: {}", path.display()));
    }

    if item.kind == Kind::Plugin
        && let Some(state) = item.plugin_state
    {
        lines.push(format!("State: {}", format_plugin_state(state)));
    }

    if matches!(item.kind, Kind::Agent | Kind::Command) && !item.extra.is_empty() {
        lines.push(String::new());
        for (key, value) in &item.extra {
            lines.push(format!("{key}: {value}"));
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use crate::model::{Item, Kind, PluginState, Source};
    use crate::ui::{
        AppState, KeyAction, classify_key, detail_lines, format_plugin_state, format_source,
        tab_title,
    };
    use crossterm::event::KeyCode;
    use std::path::PathBuf;

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

    #[test]
    fn selected_index_tracks_the_selected_row() {
        let mut state = AppState::new(vec![
            item(Kind::Agent, "one", ""),
            item(Kind::Agent, "two", ""),
        ]);
        assert_eq!(state.selected_index(), 0);
        state.next();
        assert_eq!(state.selected_index(), 1);
    }

    #[test]
    fn tab_title_maps_each_kind_to_its_display_name() {
        assert_eq!(tab_title(Kind::Agent), "Agents");
        assert_eq!(tab_title(Kind::Skill), "Skills");
        assert_eq!(tab_title(Kind::Command), "Commands");
        assert_eq!(tab_title(Kind::Plugin), "Plugins");
        assert_eq!(tab_title(Kind::Mcp), "MCP");
    }

    #[test]
    fn format_source_renders_each_variant() {
        assert_eq!(format_source(&Source::User), "user");
        assert_eq!(format_source(&Source::Project), "project");
        assert_eq!(
            format_source(&Source::Plugin {
                plugin: "superpowers".to_string(),
                marketplace: "claude-plugins-official".to_string(),
            }),
            "plugin (superpowers@claude-plugins-official)"
        );
    }

    #[test]
    fn format_plugin_state_renders_each_variant() {
        assert_eq!(format_plugin_state(PluginState::Available), "available");
        assert_eq!(format_plugin_state(PluginState::Installed), "installed");
        assert_eq!(format_plugin_state(PluginState::Enabled), "enabled");
    }

    #[test]
    fn detail_lines_includes_description_and_source_and_omits_absent_sections() {
        let mut base = item(Kind::Skill, "demo", "a demo skill");
        base.source = Source::User;
        assert_eq!(
            detail_lines(&base),
            vec![
                "a demo skill".to_string(),
                String::new(),
                "Source: user".to_string(),
            ]
        );
    }

    #[test]
    fn detail_lines_includes_path_when_the_item_has_a_backing_file() {
        let mut with_path = item(Kind::Command, "demo", "desc");
        with_path.path = Some(PathBuf::from("/tmp/demo.md"));
        assert!(detail_lines(&with_path).contains(&"Path: /tmp/demo.md".to_string()));

        let without_path = item(Kind::Command, "demo", "desc");
        assert!(
            !detail_lines(&without_path)
                .iter()
                .any(|line| line.starts_with("Path:"))
        );
    }

    #[test]
    fn detail_lines_includes_plugin_state_badge_only_for_plugin_kind() {
        let mut plugin = item(Kind::Plugin, "superpowers", "desc");
        plugin.plugin_state = Some(PluginState::Enabled);
        assert!(detail_lines(&plugin).contains(&"State: enabled".to_string()));

        let mut agent = item(Kind::Agent, "a", "desc");
        agent.plugin_state = Some(PluginState::Enabled);
        assert!(
            !detail_lines(&agent)
                .iter()
                .any(|line| line.starts_with("State:"))
        );
    }

    #[test]
    fn detail_lines_includes_extra_rows_for_agent_and_command_only() {
        let mut agent = item(Kind::Agent, "a", "desc");
        agent.extra = vec![("model".to_string(), "opus".to_string())];
        assert!(detail_lines(&agent).contains(&"model: opus".to_string()));

        let mut command = item(Kind::Command, "c", "desc");
        command.extra = vec![("model".to_string(), "opus".to_string())];
        assert!(detail_lines(&command).contains(&"model: opus".to_string()));

        let mut skill = item(Kind::Skill, "s", "desc");
        skill.extra = vec![("model".to_string(), "opus".to_string())];
        assert!(
            !detail_lines(&skill)
                .iter()
                .any(|line| line == "model: opus")
        );
    }

    #[test]
    fn classify_key_normal_mode_navigation_and_actions() {
        assert_eq!(classify_key(KeyCode::Char('q'), false), KeyAction::Quit);
        assert_eq!(classify_key(KeyCode::Esc, false), KeyAction::Quit);
        assert_eq!(classify_key(KeyCode::Tab, false), KeyAction::NextTab);
        assert_eq!(classify_key(KeyCode::BackTab, false), KeyAction::PrevTab);
        assert_eq!(classify_key(KeyCode::Char('j'), false), KeyAction::Next);
        assert_eq!(classify_key(KeyCode::Down, false), KeyAction::Next);
        assert_eq!(classify_key(KeyCode::Char('k'), false), KeyAction::Prev);
        assert_eq!(classify_key(KeyCode::Up, false), KeyAction::Prev);
        assert_eq!(
            classify_key(KeyCode::Char('/'), false),
            KeyAction::StartFilter
        );
        assert_eq!(
            classify_key(KeyCode::Char('e'), false),
            KeyAction::OpenEditor
        );
        assert_eq!(
            classify_key(KeyCode::Char('y'), false),
            KeyAction::CopyClipboard
        );
        assert_eq!(classify_key(KeyCode::Char('R'), false), KeyAction::Rescan);
        assert_eq!(classify_key(KeyCode::Char('z'), false), KeyAction::Ignore);
    }

    #[test]
    fn classify_key_filtering_mode_routes_everything_to_filter_editing() {
        assert_eq!(classify_key(KeyCode::Esc, true), KeyAction::FilterClear);
        assert_eq!(classify_key(KeyCode::Enter, true), KeyAction::FilterConfirm);
        assert_eq!(
            classify_key(KeyCode::Backspace, true),
            KeyAction::FilterBackspace
        );
        assert_eq!(
            classify_key(KeyCode::Char('R'), true),
            KeyAction::FilterChar('R')
        );
        assert_eq!(
            classify_key(KeyCode::Char('q'), true),
            KeyAction::FilterChar('q')
        );
        assert_eq!(classify_key(KeyCode::Tab, true), KeyAction::Ignore);
    }
}
