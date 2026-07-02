//! TUI rendering and event loop.
//!
//! Owns the terminal lifecycle (`ratatui::init` / `restore`), drives a
//! `tokio::select!` loop over render-tick / watcher-events / keyboard input,
//! and dispatches per-session events to the matching [`SessionEntry`].
//!
//! Two view modes:
//! - `Summary` (default): table of every discovered session, with an
//!   aggregate band and a highlighted selected row.
//! - `Single`: the legacy detail view of one session. Reachable from
//!   Summary via Enter / `s`; pinned when started with `--session`.

use anyhow::Result;
use crossterm::event::{Event as CtEvent, EventStream, KeyCode, KeyEventKind};
use futures::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Bar, BarChart, BarGroup, Block, Borders, Paragraph, Wrap},
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::jsonl::Event;
use crate::session::{self, DiscoveredSession};
use crate::stats::SessionStats;
use crate::summary::{self, AgentStatus, EntryView};
use crate::watcher;

/// Bounded mpsc channel capacity for the shared (idx, Event) bus.
/// Scales with the number of watchers; 1024 leaves plenty of headroom
/// for bursts even with dozens of sessions.
const EVENT_CHANNEL_CAPACITY: usize = 1024;
/// Maximum events drained per `recv_many` batch.
const EVENT_DRAIN_BATCH: usize = 128;
/// claude.ai page that shows subscription quota / session reset / weekly limits.
const USAGE_DASHBOARD_URL: &str = "https://claude.ai/settings/usage";

// ---- Single-view layout heights (rows). ----
const HEADER_H: u16 = 3;
const CTX_GAUGE_H: u16 = 3;
const TOKENS_H: u16 = 4;
const COST_H: u16 = 3;
const TOOLS_H: u16 = 3;
const SPARKLINE_MIN_H: u16 = 3;
const FOOTER_H: u16 = 1;

/// Runtime configuration for `ui::run`. Built once from CLI flags + env vars.
pub(crate) struct Config {
    pub projects_dir: PathBuf,
    pub explicit_session: Option<PathBuf>,
    pub refresh_ms: u64,
    pub context_window_override: Option<u64>,
    /// When `Some(n)`, the summary list also includes JSONLs touched within
    /// the last `n` days (in addition to live agents). `None` ⇒ live only.
    pub days: Option<u64>,
    /// When `Some(d)`, the session list auto-refreshes every `d` (re-running
    /// `discover_sessions` + reconciling watchers, same as manual `R`).
    /// `None` ⇒ refresh only on `R`.
    pub watch_interval: Option<Duration>,
}

/// Per-session state owned by the UI: the file path, identifiers, status +
/// name pulled from `claude agents --json`, rolling stats, and the watcher
/// task handle (so we can abort on rescan).
struct SessionEntry {
    path: PathBuf,
    short_id: String,
    session_id: String,
    name: Option<String>,
    status: AgentStatus,
    stats: SessionStats,
    handle: JoinHandle<Result<()>>,
}

/// Which view is active. Pinned to `Single` when the user passes
/// `--session <path>` (the entry list is then a single non-toggle-able row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Summary,
    Single,
}

/// Decoded keyboard intent. Pure mapping from `KeyCode` → semantic action,
/// extracted so it can be unit-tested without spinning up a terminal.
#[derive(Debug, PartialEq, Eq)]
enum KeyAction {
    Quit,
    ResetCurrent,
    OpenBrowser,
    Rescan,
    SelectPrev,
    SelectNext,
    OpenSelected, // Summary → Single on current selection
    ToggleView,   // s key
    NextInSingle, // Tab / n in single mode
    PrevInSingle, // BackTab / p in single mode
    Ignore,
}

/// Map a keypress to a semantic action. The mode (and pinned-ness) gates
/// which actions are reachable.
fn classify_key(code: KeyCode, mode: ViewMode, pinned: bool) -> KeyAction {
    match (mode, code) {
        (_, KeyCode::Char('q') | KeyCode::Esc) => KeyAction::Quit,
        (_, KeyCode::Char('b')) => KeyAction::OpenBrowser,
        (ViewMode::Summary, KeyCode::Up | KeyCode::Char('k')) => KeyAction::SelectPrev,
        (ViewMode::Summary, KeyCode::Down | KeyCode::Char('j')) => KeyAction::SelectNext,
        (ViewMode::Summary, KeyCode::Enter | KeyCode::Char('s')) => KeyAction::OpenSelected,
        (ViewMode::Summary, KeyCode::Char('R')) => KeyAction::Rescan,
        (ViewMode::Single, KeyCode::Char('s')) if !pinned => KeyAction::ToggleView,
        (ViewMode::Single, KeyCode::Char('r')) => KeyAction::ResetCurrent,
        (ViewMode::Single, KeyCode::Char('R')) if !pinned => KeyAction::Rescan,
        (ViewMode::Single, KeyCode::Tab | KeyCode::Char('n')) if !pinned => KeyAction::NextInSingle,
        (ViewMode::Single, KeyCode::BackTab | KeyCode::Char('p')) if !pinned => {
            KeyAction::PrevInSingle
        }
        _ => KeyAction::Ignore,
    }
}

/// Public entry point. Owns terminal init/restore around `run_inner`. Any
/// error that escapes propagates after the terminal has been restored.
pub(crate) async fn run(cfg: Config) -> Result<()> {
    let mut terminal = ratatui::init();
    let res = run_inner(&mut terminal, cfg).await;
    ratatui::restore();
    res
}

/// Core event loop. Drains the shared watcher bus into per-session stats,
/// redraws each tick, and handles key input per the active view mode.
async fn run_inner(terminal: &mut DefaultTerminal, cfg: Config) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::channel::<(usize, Event)>(EVENT_CHANNEL_CAPACITY);

    let (mut entries, pinned, mut mode) = if let Some(path) = cfg.explicit_session.clone() {
        // `--session <path>` mode: single pinned entry, no summary toggle.
        let short_id = session::short_id(&path);
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&short_id)
            .to_string();
        let handle = watcher::spawn(path.clone(), 0, event_tx.clone());
        let stats = SessionStats::new(cfg.context_window_override);
        let entry = SessionEntry {
            path,
            short_id,
            session_id,
            name: None,
            status: AgentStatus::Offline,
            stats,
            handle,
        };
        (vec![entry], true, ViewMode::Single)
    } else {
        let discovered = session::discover_sessions(&cfg.projects_dir, cfg.days).await?;
        if discovered.is_empty() {
            anyhow::bail!("no sessions found (start one with `claude` or widen with `--days N`)");
        }
        let entries = spawn_entries(&discovered, &event_tx, cfg.context_window_override);
        (entries, false, ViewMode::Summary)
    };

    let mut selected: usize = 0;
    let mut event_buf: Vec<(usize, Event)> = Vec::with_capacity(EVENT_DRAIN_BATCH);
    let mut tick = tokio::time::interval(Duration::from_millis(cfg.refresh_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Auto-refresh ticker. `None` when `--watch` wasn't passed; the select!
    // arm uses `future::pending` in that case so it never fires.
    let mut watch_tick = cfg.watch_interval.map(|d| {
        let mut i = tokio::time::interval(d);
        // First tick fires immediately; skip it so we don't double-rescan
        // on startup (we just discovered the list moments ago).
        i.reset();
        i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        i
    });
    let mut events = EventStream::new();

    loop {
        terminal.draw(|f| draw_frame(f, &entries, selected, mode))?;

        let watch_fut = async {
            if let Some(t) = watch_tick.as_mut() {
                let _ = t.tick().await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            _ = tick.tick() => {}
            () = watch_fut, if !pinned => {
                // Auto-refresh is best-effort: if `claude agents --json`
                // hiccups (CLI crashed, slow, network), we keep the current
                // view rather than tearing down the TUI. Manual `R` still
                // surfaces the error to the user.
                let _ = rescan(&mut entries, &event_tx, &cfg, &mut selected).await;
            }
            n = event_rx.recv_many(&mut event_buf, EVENT_DRAIN_BATCH) => {
                if n == 0 {
                    return Ok(()); // all watchers closed
                }
                for (idx, ev) in event_buf.drain(..) {
                    if let Some(entry) = entries.get_mut(idx) {
                        entry.stats.ingest(&ev);
                    }
                }
            }
            Some(Ok(ct_ev)) = events.next() => {
                if let CtEvent::Key(k) = ct_ev
                    && k.kind == KeyEventKind::Press
                {
                    match classify_key(k.code, mode, pinned) {
                        KeyAction::Quit => return Ok(()),
                        KeyAction::OpenBrowser => open_in_browser(USAGE_DASHBOARD_URL),
                        KeyAction::ResetCurrent => {
                            if let Some(e) = entries.get_mut(selected) {
                                e.stats.reset();
                            }
                        }
                        KeyAction::SelectPrev | KeyAction::PrevInSingle => {
                            selected = prev_selected(&entries, selected);
                        }
                        KeyAction::SelectNext | KeyAction::NextInSingle => {
                            selected = next_selected(&entries, selected);
                        }
                        KeyAction::OpenSelected => mode = ViewMode::Single,
                        KeyAction::ToggleView => mode = ViewMode::Summary,
                        KeyAction::Rescan => {
                            rescan(
                                &mut entries,
                                &event_tx,
                                &cfg,
                                &mut selected,
                            ).await?;
                        }
                        KeyAction::Ignore => {}
                    }
                }
            }
        }
    }
}

/// Spawn one watcher per discovered session, returning a Vec aligned by
/// index with the `(idx, Event)` tag emitted to the shared channel.
fn spawn_entries(
    discovered: &[DiscoveredSession],
    tx: &mpsc::Sender<(usize, Event)>,
    ctx_override: Option<u64>,
) -> Vec<SessionEntry> {
    discovered
        .iter()
        .enumerate()
        .map(|(idx, d)| {
            let handle = watcher::spawn(d.path.clone(), idx, tx.clone());
            let (name, status) = match &d.agent_info {
                Some(info) => (
                    info.name.clone(),
                    AgentStatus::from_kind(Some(info.status_kind)),
                ),
                None => (None, AgentStatus::Offline),
            };
            SessionEntry {
                path: d.path.clone(),
                short_id: d.short_id.clone(),
                session_id: d.session_id.clone(),
                name,
                status,
                stats: SessionStats::new(ctx_override),
                handle,
            }
        })
        .collect()
}

/// Re-discover sessions and reconcile against the running watcher set:
/// keep entries whose `session_id` still appears, abort + drop the rest,
/// spawn watchers for newly-added sessions. Preserves selection by id.
async fn rescan(
    entries: &mut Vec<SessionEntry>,
    tx: &mpsc::Sender<(usize, Event)>,
    cfg: &Config,
    selected: &mut usize,
) -> Result<()> {
    let fresh = session::discover_sessions(&cfg.projects_dir, cfg.days).await?;
    if fresh.is_empty() {
        return Ok(());
    }

    // Capture the currently-selected session id so we can re-locate it.
    let selected_id = entries.get(*selected).map(|e| e.session_id.clone());

    // Build a lookup of existing entries by session_id so we can keep their stats.
    let mut existing: HashMap<String, SessionEntry> = entries
        .drain(..)
        .map(|e| (e.session_id.clone(), e))
        .collect();

    let mut new_entries: Vec<SessionEntry> = Vec::with_capacity(fresh.len());
    for (idx, d) in fresh.iter().enumerate() {
        if let Some(mut e) = existing.remove(&d.session_id) {
            // Refresh status/name from the latest `claude agents --json`.
            let (name, status) = match &d.agent_info {
                Some(info) => (
                    info.name.clone(),
                    AgentStatus::from_kind(Some(info.status_kind)),
                ),
                None => (None, AgentStatus::Offline),
            };
            e.name = name;
            e.status = status;
            // Watcher is still running with the OLD tag (the previous index).
            // Abort it and re-spawn with the new index so dispatch stays correct.
            e.handle.abort();
            e.handle = watcher::spawn(d.path.clone(), idx, tx.clone());
            new_entries.push(e);
        } else {
            let handle = watcher::spawn(d.path.clone(), idx, tx.clone());
            let (name, status) = match &d.agent_info {
                Some(info) => (
                    info.name.clone(),
                    AgentStatus::from_kind(Some(info.status_kind)),
                ),
                None => (None, AgentStatus::Offline),
            };
            new_entries.push(SessionEntry {
                path: d.path.clone(),
                short_id: d.short_id.clone(),
                session_id: d.session_id.clone(),
                name,
                status,
                stats: SessionStats::new(cfg.context_window_override),
                handle,
            });
        }
    }

    // Anything left in `existing` was dropped from the discover_sessions output;
    // abort their watchers so we don't leak the tasks.
    for (_id, dropped) in existing {
        dropped.handle.abort();
    }

    *entries = new_entries;
    // Re-locate the previously-selected session by id, if it survived.
    *selected = selected_id
        .and_then(|id| entries.iter().position(|e| e.session_id == id))
        .unwrap_or(0);

    Ok(())
}

fn next_selected(entries: &[SessionEntry], current: usize) -> usize {
    if entries.is_empty() {
        return 0;
    }
    (current + 1) % entries.len()
}

fn prev_selected(entries: &[SessionEntry], current: usize) -> usize {
    if entries.is_empty() {
        return 0;
    }
    if current == 0 {
        entries.len() - 1
    } else {
        current - 1
    }
}

fn draw_frame(f: &mut Frame<'_>, entries: &[SessionEntry], selected: usize, mode: ViewMode) {
    match mode {
        ViewMode::Summary => {
            let views: Vec<EntryView<'_>> = entries
                .iter()
                .map(|e| EntryView {
                    stats: &e.stats,
                    agent_status: e.status,
                    name: e.name.as_deref(),
                    short_id: &e.short_id,
                    path: &e.path,
                })
                .collect();
            summary::draw(f, f.area(), &views, selected);
        }
        ViewMode::Single => {
            if let Some(entry) = entries.get(selected) {
                draw_single(f, entry, selected, entries.len());
            }
        }
    }
}

/// Top-level render for the single-session detail view. Same layout
/// as before the multi-session refactor — only the data source moved.
fn draw_single(f: &mut Frame<'_>, entry: &SessionEntry, idx: usize, total: usize) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_H),
            Constraint::Length(CTX_GAUGE_H),
            Constraint::Length(TOKENS_H),
            Constraint::Length(COST_H),
            Constraint::Length(TOOLS_H),
            Constraint::Min(SPARKLINE_MIN_H),
            Constraint::Length(FOOTER_H),
        ])
        .split(area);

    draw_header(f, chunks[0], entry, idx, total);
    draw_ctx_gauge(f, chunks[1], &entry.stats);
    draw_tokens(f, chunks[2], &entry.stats);
    draw_cost(f, chunks[3], &entry.stats);
    draw_tools(f, chunks[4], &entry.stats);
    draw_sparkline(f, chunks[5], &entry.stats);
    draw_single_footer(f, chunks[6], total > 1);
}

fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::default().borders(Borders::ALL).title(title)
}

fn draw_single_footer(f: &mut Frame<'_>, area: Rect, multi: bool) {
    let mut spans: Vec<Span<'_>> = vec![Span::raw(" ")];
    let push_key = |spans: &mut Vec<Span<'_>>, key: &'static str, desc: &'static str| {
        if spans.len() > 1 {
            spans.push(Span::styled("  ·  ", dim()));
        }
        spans.push(Span::styled(key, bold()));
        spans.push(Span::styled(format!(" {desc}"), dim()));
    };
    push_key(&mut spans, "q", "quit");
    push_key(&mut spans, "r", "reset");
    push_key(&mut spans, "b", "browser");
    if multi {
        push_key(&mut spans, "s", "summary");
        push_key(&mut spans, "Tab/n", "next");
        push_key(&mut spans, "⇧Tab/p", "prev");
        push_key(&mut spans, "R", "rescan");
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_header(f: &mut Frame<'_>, area: Rect, entry: &SessionEntry, idx: usize, total: usize) {
    let s = &entry.stats;
    let raw_model = s.model_raw().unwrap_or("(waiting…)");
    let model = raw_model.strip_prefix("claude-").unwrap_or(raw_model);
    let elapsed = format_duration(s.elapsed_secs());
    let project = s.project_basename().unwrap_or("(unknown)");
    let dot_color = if s.is_active() {
        Color::Green
    } else {
        Color::Yellow
    };

    let mut spans = vec![
        Span::styled("● ", Style::default().fg(dot_color)),
        Span::styled("model: ", dim()),
        Span::styled(model.to_string(), bold_fg(s.model_color())),
        Span::raw("   "),
        Span::styled("project: ", dim()),
        Span::styled(project.to_string(), bold_fg(Color::Green)),
        Span::raw("   "),
        Span::styled("session: ", dim()),
        Span::styled(entry.short_id.clone(), bold()),
        Span::raw("   "),
        Span::styled("elapsed: ", dim()),
        Span::styled(elapsed, bold()),
        Span::raw("   "),
        Span::styled("msgs: ", dim()),
        Span::styled(s.assistant_messages().to_string(), bold()),
    ];
    if total > 1 {
        spans.push(Span::raw("   "));
        spans.push(Span::styled("[", dim()));
        spans.push(Span::styled(format!("{}/{}", idx + 1, total), bold()));
        spans.push(Span::styled("]", dim()));
    }
    if let Some(name) = entry.name.as_deref() {
        spans.push(Span::raw("   "));
        spans.push(Span::styled("name: ", dim()));
        spans.push(Span::styled(name.to_string(), bold()));
    }

    let title = match s.cwd() {
        Some(cwd) => format!(" ccwatch — {cwd} "),
        None => " ccwatch ".to_string(),
    };
    f.render_widget(Paragraph::new(Line::from(spans)).block(panel(title)), area);
}

fn draw_ctx_gauge(f: &mut Frame<'_>, area: Rect, s: &SessionStats) {
    let pct = s.context_pct();
    let used = fmt_int(s.last_context_size());
    let total = fmt_int(s.context_window());
    let label = format!("  {used} / {total}  ({:.1}%)", pct * 100.0);
    let filled_color = threshold_color(pct, 0.6, 0.8);

    let inner_width = area.width.saturating_sub(2);
    let label_cells = u16::try_from(label.chars().count()).unwrap_or(u16::MAX);
    let bars_cells = inner_width.saturating_sub(label_cells);
    let bar_count = (bars_cells / 2) as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = ((pct * bar_count as f64).round() as usize).min(bar_count);

    let mut spans: Vec<Span<'_>> = Vec::with_capacity(bar_count * 2 + 1);
    for i in 0..bar_count {
        let style = if i < filled {
            Style::default().fg(filled_color)
        } else {
            dim()
        };
        spans.push(Span::styled("█", style));
        if i + 1 < bar_count {
            spans.push(Span::raw(" "));
        }
    }
    spans.push(Span::raw(label));

    f.render_widget(
        Paragraph::new(Line::from(spans)).block(panel(" context ")),
        area,
    );
}

fn draw_tokens(f: &mut Frame<'_>, area: Rect, s: &SessionStats) {
    let totals = s.totals();
    let hit = s.cache_hit_ratio();
    let hit_color = threshold_color(1.0 - hit, 0.2, 0.5);
    let lines = vec![
        Line::from(vec![
            Span::styled("in:       ", dim()),
            value_span(fmt_int(totals.input_tokens), Color::White),
            Span::raw("    "),
            Span::styled("out:      ", dim()),
            value_span(fmt_int(totals.output_tokens), Color::White),
        ]),
        Line::from(vec![
            Span::styled("cache_w:  ", dim()),
            value_span(fmt_int(totals.cache_creation_input_tokens), Color::Cyan),
            Span::raw("    "),
            Span::styled("cache_r:  ", dim()),
            value_span(fmt_int(totals.cache_read_input_tokens), Color::Cyan),
            Span::raw("    "),
            Span::styled("hit: ", dim()),
            value_span(format!("{:.1}%", hit * 100.0), hit_color),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).block(panel(" tokens ")), area);
}

fn draw_cost(f: &mut Frame<'_>, area: Rect, s: &SessionStats) {
    let cph = s.cost_per_hour();
    let burn_color = threshold_color(cph, 5.0, 15.0);
    let line = Line::from(vec![
        Span::styled("session: ", dim()),
        value_span(format!("${:.2}", s.session_cost_usd()), Color::White),
        Span::raw("    "),
        Span::styled("rate: ", dim()),
        value_span(
            format!("{:.0} tok/min", s.tokens_per_minute()),
            Color::White,
        ),
        Span::raw("    "),
        Span::styled("burn: ", dim()),
        value_span(format!("${cph:.2}/hr"), burn_color),
    ]);
    f.render_widget(Paragraph::new(line).block(panel(" cost ")), area);
}

fn draw_tools(f: &mut Frame<'_>, area: Rect, s: &SessionStats) {
    let tools = s.tools_sorted();
    let mut spans: Vec<Span> = Vec::with_capacity(tools.len() * 3);
    if tools.is_empty() {
        spans.push(Span::styled("(no tool calls yet)", dim()));
    } else {
        for (i, (name, count)) in tools.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled((*name).to_string(), bold_fg(tool_color(name))));
            spans.push(Span::styled(format!(":{count}"), dim()));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans))
            .wrap(Wrap { trim: true })
            .block(panel(" tools ")),
        area,
    );
}

fn draw_sparkline(f: &mut Frame<'_>, area: Rect, s: &SessionStats) {
    let history = s.output_history();
    let inner_width = area.width.saturating_sub(2);
    let slot_count = (inner_width / 2) as usize;

    let visible_start = history.len().saturating_sub(slot_count);
    let visible = &history[visible_start..];
    let pad_count = slot_count.saturating_sub(history.len());

    let mut bars: Vec<Bar<'_>> = Vec::with_capacity(slot_count);
    for _ in 0..pad_count {
        bars.push(Bar::default().value(0));
    }
    for &v in visible {
        bars.push(
            Bar::default()
                .value(v)
                .style(Style::default().fg(Color::Magenta)),
        );
    }

    let title = if history.len() > slot_count {
        format!(
            " output tokens / message  (latest {} of {}) ",
            visible.len(),
            history.len()
        )
    } else {
        format!(" output tokens / message  ({}) ", history.len())
    };

    f.render_widget(
        BarChart::default()
            .block(panel(title))
            .data(BarGroup::default().bars(&bars))
            .bar_width(1)
            .bar_gap(1)
            .bar_style(Style::default().fg(Color::Magenta)),
        area,
    );
}

fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let _ = std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ---- style helpers ----

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn bold_fg(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}

fn value_span<S: Into<String>>(s: S, color: Color) -> Span<'static> {
    Span::styled(s.into(), bold_fg(color))
}

#[must_use]
fn threshold_color(value: f64, warn: f64, crit: f64) -> Color {
    if value < warn {
        Color::Green
    } else if value < crit {
        Color::Yellow
    } else {
        Color::Red
    }
}

#[must_use]
fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[must_use]
fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

#[must_use]
fn tool_color(name: &str) -> Color {
    match name {
        "Bash" => Color::Cyan,
        "Edit" => Color::Yellow,
        "Read" => Color::Blue,
        "Grep" | "Glob" => Color::Magenta,
        "Write" => Color::Green,
        "Agent" | "Task" => Color::Red,
        "WebFetch" | "WebSearch" => Color::LightBlue,
        _ => Color::White,
    }
}

// Keep Path import alive for any future helpers; currently unused at the
// function level but referenced in module-level docs.
#[allow(dead_code)]
fn _unused_path_marker(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_key_quit_always_works() {
        assert_eq!(
            classify_key(KeyCode::Char('q'), ViewMode::Summary, false),
            KeyAction::Quit
        );
        assert_eq!(
            classify_key(KeyCode::Esc, ViewMode::Single, true),
            KeyAction::Quit
        );
    }

    #[test]
    fn classify_key_summary_select_arrows_and_vim() {
        assert_eq!(
            classify_key(KeyCode::Up, ViewMode::Summary, false),
            KeyAction::SelectPrev
        );
        assert_eq!(
            classify_key(KeyCode::Char('k'), ViewMode::Summary, false),
            KeyAction::SelectPrev
        );
        assert_eq!(
            classify_key(KeyCode::Down, ViewMode::Summary, false),
            KeyAction::SelectNext
        );
        assert_eq!(
            classify_key(KeyCode::Char('j'), ViewMode::Summary, false),
            KeyAction::SelectNext
        );
    }

    #[test]
    fn classify_key_open_selected_via_enter_or_s() {
        assert_eq!(
            classify_key(KeyCode::Enter, ViewMode::Summary, false),
            KeyAction::OpenSelected
        );
        assert_eq!(
            classify_key(KeyCode::Char('s'), ViewMode::Summary, false),
            KeyAction::OpenSelected
        );
    }

    #[test]
    fn classify_key_single_s_toggles_unless_pinned() {
        assert_eq!(
            classify_key(KeyCode::Char('s'), ViewMode::Single, false),
            KeyAction::ToggleView
        );
        assert_eq!(
            classify_key(KeyCode::Char('s'), ViewMode::Single, true),
            KeyAction::Ignore
        );
    }

    #[test]
    fn classify_key_single_nav_requires_unpinned() {
        assert_eq!(
            classify_key(KeyCode::Tab, ViewMode::Single, false),
            KeyAction::NextInSingle
        );
        assert_eq!(
            classify_key(KeyCode::Tab, ViewMode::Single, true),
            KeyAction::Ignore
        );
        assert_eq!(
            classify_key(KeyCode::BackTab, ViewMode::Single, false),
            KeyAction::PrevInSingle
        );
    }

    #[test]
    fn classify_key_reset_only_in_single() {
        assert_eq!(
            classify_key(KeyCode::Char('r'), ViewMode::Single, false),
            KeyAction::ResetCurrent
        );
        // 'r' in summary mode is unbound — selection refresh isn't a thing.
        assert_eq!(
            classify_key(KeyCode::Char('r'), ViewMode::Summary, false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn classify_key_rescan_blocked_when_pinned() {
        assert_eq!(
            classify_key(KeyCode::Char('R'), ViewMode::Summary, false),
            KeyAction::Rescan
        );
        assert_eq!(
            classify_key(KeyCode::Char('R'), ViewMode::Single, false),
            KeyAction::Rescan
        );
        assert_eq!(
            classify_key(KeyCode::Char('R'), ViewMode::Single, true),
            KeyAction::Ignore
        );
    }

    #[test]
    fn threshold_color_picks_band() {
        assert_eq!(threshold_color(0.1, 0.6, 0.8), Color::Green);
        assert_eq!(threshold_color(0.7, 0.6, 0.8), Color::Yellow);
        assert_eq!(threshold_color(0.9, 0.6, 0.8), Color::Red);
    }

    #[test]
    fn fmt_int_inserts_thousand_separators() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(1_234_567), "1,234,567");
    }

    #[test]
    fn format_duration_omits_leading_zero_units() {
        assert_eq!(format_duration(7), "7s");
        assert_eq!(format_duration(65), "1m05s");
        assert_eq!(format_duration(3661), "1h01m01s");
    }
}
