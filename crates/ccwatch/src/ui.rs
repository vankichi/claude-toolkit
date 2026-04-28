//! TUI rendering and event loop.
//!
//! Owns the terminal lifecycle (`ratatui::init` / `restore`), drives a
//! `tokio::select!` loop over render-tick / watcher-events / keyboard input,
//! and renders the layout into block-style panels.

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
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::jsonl::Event;
use crate::session;
use crate::stats::SessionStats;
use crate::watcher;

/// Bounded mpsc channel capacity for events flowing watcher → UI.
const EVENT_CHANNEL_CAPACITY: usize = 256;
/// Maximum events drained per `recv_many` batch (cap on per-tick ingest).
const EVENT_DRAIN_BATCH: usize = 64;
/// claude.ai page that shows subscription quota / session reset / weekly limits.
/// Opened by the `b` key handler; not parsed by ccwatch (no API access).
const USAGE_DASHBOARD_URL: &str = "https://claude.ai/settings/usage";

// ---- Layout heights (rows). Sum + Min(SPARKLINE_MIN_H) + footer is the frame. ----
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
}

/// Snapshot of state passed to the renderer for one frame. Borrowed view
/// over the live `SessionStats` plus minimal session-list metadata.
struct RenderCtx<'a> {
    stats: &'a SessionStats,
    session_short_id: &'a str,
    /// `(current_index, total_sessions)` — None when locked to a single session.
    session_position: Option<(usize, usize)>,
}

/// Per-session runtime: an mpsc receiver fed by a spawned watcher task,
/// the abortable `JoinHandle`, the running aggregates, and the cached short id.
/// Exists for the lifetime of one session view; replaced on Tab/p/R.
struct ActiveSession {
    rx: mpsc::Receiver<Event>,
    handle: tokio::task::JoinHandle<Result<()>>,
    stats: SessionStats,
    short_id: String,
}

impl ActiveSession {
    /// Start watching `path`: spawn the tail task, build a fresh
    /// `SessionStats` (with the user-supplied context window override), and
    /// cache the short session id for the header.
    fn spawn(path: &Path, override_ctx: Option<u64>) -> Self {
        let (tx, rx) = mpsc::channel::<Event>(EVENT_CHANNEL_CAPACITY);
        let handle = watcher::spawn(path.to_path_buf(), tx);
        Self {
            rx,
            handle,
            stats: SessionStats::new(override_ctx),
            short_id: session::short_id(path),
        }
    }

    /// Stop the spawned watcher task. The channel sender drops, the consumer
    /// drains remaining buffered events, then exits cleanly.
    fn shutdown(&self) {
        self.handle.abort();
    }
}

/// Multi-session navigator. Holds the discovered JSONL paths (mtime
/// descending) and the index currently displayed. `multi` is false when
/// `--session` pinned us to a single file (Tab/p/R disabled in that mode).
struct SessionList {
    paths: Vec<PathBuf>,
    current_idx: usize,
    multi: bool,
}

impl SessionList {
    /// Initial population: either a single explicit session (single mode) or
    /// the result of scanning `projects_dir` (multi mode). Errors when
    /// scanning yields zero sessions — there's nothing to display.
    fn build(cfg: &Config) -> Result<Self> {
        if let Some(p) = cfg.explicit_session.clone() {
            return Ok(Self {
                paths: vec![p],
                current_idx: 0,
                multi: false,
            });
        }
        let paths = session::list_all_sessions(&cfg.projects_dir)?;
        if paths.is_empty() {
            anyhow::bail!(
                "no JSONL session files found under {}",
                cfg.projects_dir.display()
            );
        }
        Ok(Self {
            paths,
            current_idx: 0,
            multi: true,
        })
    }

    /// Current session's path on disk.
    fn current(&self) -> &Path {
        &self.paths[self.current_idx]
    }

    /// `(idx, total)` for the header `[N/M]` indicator. None in single-session mode.
    fn position(&self) -> Option<(usize, usize)> {
        self.multi.then_some((self.current_idx, self.paths.len()))
    }

    /// Cycle forward, wrapping to 0 at the end.
    fn next(&mut self) {
        self.current_idx = (self.current_idx + 1) % self.paths.len();
    }

    /// Cycle backward, wrapping to last at 0.
    fn prev(&mut self) {
        self.current_idx = if self.current_idx == 0 {
            self.paths.len() - 1
        } else {
            self.current_idx - 1
        };
    }

    /// Re-scan `projects_dir`. Preserves the displayed session when its file
    /// still exists; otherwise falls back to index 0 (newest).
    fn rescan(&mut self, projects_dir: &Path) -> Result<()> {
        let fresh = session::list_all_sessions(projects_dir)?;
        if fresh.is_empty() {
            return Ok(());
        }
        let current_path = self.paths[self.current_idx].clone();
        self.paths = fresh;
        self.current_idx = self
            .paths
            .iter()
            .position(|p| *p == current_path)
            .unwrap_or(0);
        Ok(())
    }
}

/// Decoded keyboard intent. Pure mapping from `KeyCode` → semantic action,
/// extracted out of the event loop so it can be unit-tested.
#[derive(Debug, PartialEq, Eq)]
enum KeyAction {
    Quit,
    ResetCurrent,
    Next,
    Prev,
    Rescan,
    OpenBrowser,
    Ignore,
}

/// Map a single keypress to a `KeyAction`. `multi_session` gates navigation
/// and rescan keys (they're meaningless when pinned to one session).
fn classify_key(code: KeyCode, multi_session: bool) -> KeyAction {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => KeyAction::Quit,
        KeyCode::Char('r') => KeyAction::ResetCurrent,
        KeyCode::Char('b') => KeyAction::OpenBrowser,
        KeyCode::Tab | KeyCode::Char('n') if multi_session => KeyAction::Next,
        KeyCode::BackTab | KeyCode::Char('p') if multi_session => KeyAction::Prev,
        KeyCode::Char('R') if multi_session => KeyAction::Rescan,
        _ => KeyAction::Ignore,
    }
}

/// Spawn the OS-default browser opener pointing at `url`. Fire-and-forget:
/// returns immediately, ignores any failure so a missing `open`/`xdg-open`
/// command can't crash the TUI.
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

/// Tear down the current `ActiveSession` and start watching a different path.
/// Used by all three navigation actions (Next/Prev/Rescan).
fn switch_active(active: &mut ActiveSession, path: &Path, override_ctx: Option<u64>) {
    active.shutdown();
    *active = ActiveSession::spawn(path, override_ctx);
}

/// Public entry point. Owns terminal init/restore around `run_inner`. Any
/// error that escapes propagates after the terminal has been restored.
pub(crate) async fn run(cfg: Config) -> Result<()> {
    let mut terminal = ratatui::init();
    let res = run_inner(&mut terminal, cfg).await;
    ratatui::restore();
    res
}

/// Core event loop. Drains watcher events into stats, redraws on each tick,
/// and handles keypresses (quit / reset / session switch / rescan).
async fn run_inner(terminal: &mut DefaultTerminal, cfg: Config) -> Result<()> {
    let mut sessions = SessionList::build(&cfg)?;
    let mut active = ActiveSession::spawn(sessions.current(), cfg.context_window_override);

    let mut tick = tokio::time::interval(Duration::from_millis(cfg.refresh_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut events = EventStream::new();
    let mut event_buf: Vec<Event> = Vec::with_capacity(EVENT_DRAIN_BATCH);

    loop {
        let position = sessions.position();
        terminal.draw(|f| {
            draw(
                f,
                &RenderCtx {
                    stats: &active.stats,
                    session_short_id: &active.short_id,
                    session_position: position,
                },
            );
        })?;

        tokio::select! {
            _ = tick.tick() => {}
            n = active.rx.recv_many(&mut event_buf, EVENT_DRAIN_BATCH) => {
                if n == 0 {
                    return Ok(()); // watcher channel closed
                }
                for ev in event_buf.drain(..) {
                    active.stats.ingest(&ev);
                }
            }
            Some(Ok(ct_ev)) = events.next() => {
                if let CtEvent::Key(k) = ct_ev
                    && k.kind == KeyEventKind::Press
                {
                    match classify_key(k.code, sessions.multi) {
                        KeyAction::Quit => return Ok(()),
                        KeyAction::ResetCurrent => active.stats.reset(),
                        KeyAction::OpenBrowser => open_in_browser(USAGE_DASHBOARD_URL),
                        KeyAction::Next => {
                            sessions.next();
                            switch_active(&mut active, sessions.current(), cfg.context_window_override);
                        }
                        KeyAction::Prev => {
                            sessions.prev();
                            switch_active(&mut active, sessions.current(), cfg.context_window_override);
                        }
                        KeyAction::Rescan => {
                            sessions.rescan(&cfg.projects_dir)?;
                            switch_active(&mut active, sessions.current(), cfg.context_window_override);
                        }
                        KeyAction::Ignore => {}
                    }
                }
            }
        }
    }
}

/// Top-level render: split the frame into fixed-height bands and dispatch
/// each band to a dedicated `draw_*` function. The sparkline gets the
/// remaining space (`Min(SPARKLINE_MIN_H)`); footer is pinned to the last row.
fn draw(f: &mut Frame<'_>, ctx: &RenderCtx<'_>) {
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

    draw_header(f, chunks[0], ctx);
    draw_ctx_gauge(f, chunks[1], ctx.stats);
    draw_tokens(f, chunks[2], ctx.stats);
    draw_cost(f, chunks[3], ctx.stats);
    draw_tools(f, chunks[4], ctx.stats);
    draw_sparkline(f, chunks[5], ctx.stats);
    draw_footer(f, chunks[6], ctx);
}

/// Standard panel chrome: full borders and a dim title. All `draw_*` panels
/// share the same look, so this is the single source of truth.
fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::default().borders(Borders::ALL).title(title)
}

/// Single-row hint line at the bottom showing available key bindings.
/// Hides multi-session-only keys when pinned to a single session.
fn draw_footer(f: &mut Frame<'_>, area: Rect, ctx: &RenderCtx<'_>) {
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
    if ctx.session_position.is_some() {
        push_key(&mut spans, "Tab/n", "next");
        push_key(&mut spans, "⇧Tab/p", "prev");
        push_key(&mut spans, "R", "rescan");
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Title-bar style header: active dot, model name (color-coded by family),
/// project basename, short session id, elapsed time, message count, and
/// `[idx/total]` position when multiple sessions are available. The block's
/// title carries the full cwd for easy disambiguation across projects.
fn draw_header(f: &mut Frame<'_>, area: Rect, ctx: &RenderCtx<'_>) {
    let s = ctx.stats;
    let model = s.model_raw().unwrap_or("(waiting…)");
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
        Span::styled(ctx.session_short_id.to_string(), bold()),
        Span::raw("   "),
        Span::styled("elapsed: ", dim()),
        Span::styled(elapsed, bold()),
        Span::raw("   "),
        Span::styled("msgs: ", dim()),
        Span::styled(s.assistant_messages().to_string(), bold()),
    ];
    if let Some((idx, total)) = ctx.session_position
        && total > 1
    {
        spans.push(Span::raw("   "));
        spans.push(Span::styled("[", dim()));
        spans.push(Span::styled(format!("{}/{}", idx + 1, total), bold()));
        spans.push(Span::styled("]", dim()));
    }

    let title = match s.cwd() {
        Some(cwd) => format!(" ccwatch — {cwd} "),
        None => " ccwatch ".to_string(),
    };
    f.render_widget(Paragraph::new(Line::from(spans)).block(panel(title)), area);
}

/// Discrete-block gauge for context window usage. Fills `█ █ █` slots from
/// the left up to `pct`, with unfilled slots dim. Color follows
/// `threshold_color` (green → yellow → red as fullness rises). Appends an
/// `auto` marker when the 1M context size was inferred from observed data.
fn draw_ctx_gauge(f: &mut Frame<'_>, area: Rect, s: &SessionStats) {
    let pct = s.context_pct();
    let used = fmt_int(s.last_context_size());
    let total = fmt_int(s.context_window());
    let auto_marker = if s.is_auto_promoted() { " auto" } else { "" };
    let label = format!("  {used} / {total}{auto_marker}  ({:.1}%)", pct * 100.0);
    let filled_color = threshold_color(pct, 0.6, 0.8);

    // Inner width = area.width - 2 (borders). Each bar slot is 2 cells (block + gap).
    // Reserve space at the right for the numeric label.
    let inner_width = area.width.saturating_sub(2);
    let label_cells = u16::try_from(label.chars().count()).unwrap_or(u16::MAX);
    let bars_cells = inner_width.saturating_sub(label_cells);
    let bar_count = (bars_cells / 2) as usize;
    // Safe: pct ∈ [0, 1] is clamped upstream, bar_count is bounded by terminal width.
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

/// Two-line cumulative token panel: `in / out` on the first line,
/// `cache_w / cache_r / hit%` on the second. Cache-hit color uses the
/// inverted threshold (high hit = green, low hit = red).
fn draw_tokens(f: &mut Frame<'_>, area: Rect, s: &SessionStats) {
    let totals = s.totals();
    let hit = s.cache_hit_ratio();
    // hit-ratio: higher is better, so invert thresholds (1 - hit).
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

/// Cost row: cumulative session spend, sliding-window throughput
/// (`tokens/min`), and burn rate (`$/hr`). Burn rate color flags runaway
/// spending (red above $15/hr).
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

/// Tool-call breakdown: each tool name in its accent color (see
/// `tool_color`) followed by its count, sorted by descending frequency.
/// Wraps onto multiple lines if the panel is narrow.
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

/// Per-message output-token bar chart. Right-aligns the most recent N bars
/// that fit in the panel width (older entries scroll off the left). Empty
/// padding bars are inserted on the left when history is shorter than the
/// available slot count, so the newest bar always sits at the right edge.
fn draw_sparkline(f: &mut Frame<'_>, area: Rect, s: &SessionStats) {
    let history = s.output_history();
    // Each bar slot is 2 cells (bar_width=1 + bar_gap=1). Subtract borders.
    let inner_width = area.width.saturating_sub(2);
    let slot_count = (inner_width / 2) as usize;

    let visible_start = history.len().saturating_sub(slot_count);
    let visible = &history[visible_start..];
    let pad_count = slot_count.saturating_sub(history.len());

    // Left-pad with zero-value bars so the visible (most recent) bars right-align.
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

// ---- style helpers ----

/// Dim foreground (dark gray). Used for labels, separators, and unfilled bar slots.
fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Default-foreground bold. Used for emphasized inline values.
fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Bold + colored foreground. Used for accented values (model name, project, etc.).
fn bold_fg(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}

/// Owned-string span styled with `bold_fg`. Convenience for dynamic numeric values.
fn value_span<S: Into<String>>(s: S, color: Color) -> Span<'static> {
    Span::styled(s.into(), bold_fg(color))
}

/// Map a value to green/yellow/red by two ascending thresholds.
/// `value < warn` → green, `value < crit` → yellow, otherwise red.
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

/// Format a `u64` with comma thousand-separators (e.g. `1234567` → `"1,234,567"`).
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

/// Compact `h{m}m{s}s` duration formatter that drops leading zero-units.
/// E.g. `7s`, `1m05s`, `1h01m01s`.
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

/// Stable per-tool color. Unknown tool names fall through to white. Updates
/// here change all tool labels in the `tools` panel.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_key_maps_quit_keys() {
        assert_eq!(classify_key(KeyCode::Char('q'), false), KeyAction::Quit);
        assert_eq!(classify_key(KeyCode::Esc, false), KeyAction::Quit);
        assert_eq!(classify_key(KeyCode::Char('q'), true), KeyAction::Quit);
    }

    #[test]
    fn classify_key_reset_works_in_both_modes() {
        assert_eq!(
            classify_key(KeyCode::Char('r'), false),
            KeyAction::ResetCurrent
        );
        assert_eq!(
            classify_key(KeyCode::Char('r'), true),
            KeyAction::ResetCurrent
        );
    }

    #[test]
    fn classify_key_b_opens_browser_in_both_modes() {
        assert_eq!(
            classify_key(KeyCode::Char('b'), false),
            KeyAction::OpenBrowser
        );
        assert_eq!(
            classify_key(KeyCode::Char('b'), true),
            KeyAction::OpenBrowser
        );
    }

    #[test]
    fn classify_key_navigation_requires_multi_session() {
        // Single-session mode: nav keys are ignored.
        assert_eq!(classify_key(KeyCode::Tab, false), KeyAction::Ignore);
        assert_eq!(classify_key(KeyCode::Char('n'), false), KeyAction::Ignore);
        assert_eq!(classify_key(KeyCode::BackTab, false), KeyAction::Ignore);
        assert_eq!(classify_key(KeyCode::Char('p'), false), KeyAction::Ignore);
        assert_eq!(classify_key(KeyCode::Char('R'), false), KeyAction::Ignore);
        // Multi-session mode: they map to navigation actions.
        assert_eq!(classify_key(KeyCode::Tab, true), KeyAction::Next);
        assert_eq!(classify_key(KeyCode::Char('n'), true), KeyAction::Next);
        assert_eq!(classify_key(KeyCode::BackTab, true), KeyAction::Prev);
        assert_eq!(classify_key(KeyCode::Char('p'), true), KeyAction::Prev);
        assert_eq!(classify_key(KeyCode::Char('R'), true), KeyAction::Rescan);
    }

    #[test]
    fn classify_key_unknown_keys_ignored() {
        assert_eq!(classify_key(KeyCode::Char('x'), true), KeyAction::Ignore);
        assert_eq!(classify_key(KeyCode::F(5), true), KeyAction::Ignore);
    }

    #[test]
    fn threshold_color_picks_band() {
        assert_eq!(threshold_color(0.1, 0.6, 0.8), Color::Green);
        assert_eq!(threshold_color(0.7, 0.6, 0.8), Color::Yellow);
        assert_eq!(threshold_color(0.9, 0.6, 0.8), Color::Red);
        assert_eq!(threshold_color(0.6, 0.6, 0.8), Color::Yellow); // boundary
    }

    #[test]
    fn fmt_int_inserts_thousand_separators() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1_000), "1,000");
        assert_eq!(fmt_int(12_345), "12,345");
        assert_eq!(fmt_int(1_234_567), "1,234,567");
    }

    #[test]
    fn format_duration_omits_leading_zero_units() {
        assert_eq!(format_duration(7), "7s");
        assert_eq!(format_duration(65), "1m05s");
        assert_eq!(format_duration(3661), "1h01m01s");
    }
}
