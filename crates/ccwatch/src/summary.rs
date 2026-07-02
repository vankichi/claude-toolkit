//! Summary view — multi-session table + aggregate band.
//!
//! Renders one row per discovered session, with a header band showing
//! live count, total cost, total messages, and a per-model breakdown.
//! Pure helpers (`compute_aggregate`, `sort_entries`) are extracted so
//! they can be unit-tested without a terminal.

use crate::session::AgentStatusKind;
use crate::stats::SessionStats;
use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Status used for the `status` column. `Offline` is for historic sessions
/// surfaced via `--days` that aren't currently in `claude agents --json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentStatus {
    Busy,
    Idle,
    Offline,
}

impl AgentStatus {
    pub(crate) fn from_kind(k: Option<AgentStatusKind>) -> Self {
        match k {
            Some(AgentStatusKind::Busy) => Self::Busy,
            Some(AgentStatusKind::Idle) => Self::Idle,
            None => Self::Offline,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Idle => "idle",
            Self::Offline => "off",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Busy => Color::Green,
            Self::Idle => Color::Yellow,
            Self::Offline => Color::DarkGray,
        }
    }

    fn dot(self) -> &'static str {
        match self {
            Self::Busy | Self::Idle => "●",
            Self::Offline => "○",
        }
    }
}

/// One row in the summary table. Borrowed view over a `SessionEntry` —
/// the UI owns the entries, this just describes the renderable shape.
pub(crate) struct EntryView<'a> {
    pub stats: &'a SessionStats,
    pub agent_status: AgentStatus,
    pub name: Option<&'a str>,
    pub short_id: &'a str,
    pub path: &'a PathBuf,
}

/// Aggregate band content. Computed from the visible entries on every
/// frame. Cheap — sums + a small `BTreeMap` over the model breakdown.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct AggregateRow {
    pub live_count: usize,
    pub total_cost_usd: f64,
    pub total_msgs: u64,
    /// `("opus", 2)` style buckets, sorted by family name for stable rendering.
    pub model_breakdown: Vec<(String, usize)>,
}

/// Roll up visible entries into the band's totals. Pure function — pulled
/// out for unit testing.
#[must_use]
pub(crate) fn compute_aggregate(entries: &[EntryView<'_>]) -> AggregateRow {
    let mut total_cost = 0.0_f64;
    let mut total_msgs = 0_u64;
    let mut live_count = 0_usize;
    let mut buckets: BTreeMap<String, usize> = BTreeMap::new();

    for e in entries {
        total_cost += e.stats.session_cost_usd();
        total_msgs += e.stats.assistant_messages();
        if e.agent_status != AgentStatus::Offline {
            live_count += 1;
        }
        let family = model_family(e.stats.model_raw());
        *buckets.entry(family).or_insert(0) += 1;
    }

    AggregateRow {
        live_count,
        total_cost_usd: total_cost,
        total_msgs,
        model_breakdown: buckets.into_iter().collect(),
    }
}

/// Sort key for [`sort_entries`]. Most recent activity first; entries
/// without events fall back to the file mtime. Stable for ties.
fn sort_key(stats: &SessionStats, path: &PathBuf) -> (Option<DateTime<Utc>>, SystemTime) {
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    (stats.last_event_at(), mtime)
}

/// Sort an index Vec into `entries` descending by recent activity (events
/// first, mtime as tiebreaker for cold sessions). Returns indices because
/// the UI's `SessionList` owns the entry storage and we only reorder a view.
#[must_use]
pub(crate) fn sort_order(entries: &[EntryView<'_>]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..entries.len()).collect();
    idx.sort_by(|&a, &b| {
        let ka = sort_key(entries[a].stats, entries[a].path);
        let kb = sort_key(entries[b].stats, entries[b].path);
        // Descending: newer last_event_at first; None goes to the end.
        match (ka.0, kb.0) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => kb.1.cmp(&ka.1),
        }
    });
    idx
}

/// Render the summary view into `area`. Two-band layout: aggregate row on
/// top, table below. The selected row is highlighted via REVERSED style.
pub(crate) fn draw(f: &mut Frame<'_>, area: Rect, entries: &[EntryView<'_>], selected: usize) {
    use ratatui::layout::{Direction, Layout};

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // aggregate band
            Constraint::Min(5),    // table
            Constraint::Length(1), // footer
        ])
        .split(area);

    draw_aggregate(f, chunks[0], entries);
    draw_table(f, chunks[1], entries, selected);
    draw_footer(f, chunks[2]);
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}
fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
fn bold_fg(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}

fn draw_aggregate(f: &mut Frame<'_>, area: Rect, entries: &[EntryView<'_>]) {
    let agg = compute_aggregate(entries);
    let mut spans: Vec<Span<'_>> = vec![
        Span::raw(" "),
        Span::styled("live: ", dim()),
        Span::styled(agg.live_count.to_string(), bold()),
        Span::raw("   "),
        Span::styled("total: ", dim()),
        Span::styled(format!("${:.2}", agg.total_cost_usd), bold_fg(Color::White)),
        Span::raw("   "),
        Span::styled("msgs: ", dim()),
        Span::styled(agg.total_msgs.to_string(), bold()),
    ];
    if !agg.model_breakdown.is_empty() {
        spans.push(Span::raw("   "));
        for (i, (family, count)) in agg.model_breakdown.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(family.clone(), bold_fg(family_color(family))));
            spans.push(Span::styled(format!(":{count}"), dim()));
        }
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" ccwatch — summary ");
    f.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn draw_table(f: &mut Frame<'_>, area: Rect, entries: &[EntryView<'_>], selected: usize) {
    let header_cells = [
        "status", "project", "name", "model", "mode", "msgs", "ctx", "session",
    ]
    .iter()
    .map(|h| Cell::from(Span::styled((*h).to_string(), dim())));
    let header = Row::new(header_cells).height(1);

    let order = sort_order(entries);
    let rows: Vec<Row<'_>> = order
        .iter()
        .map(|&idx| {
            let e = &entries[idx];
            let s = e.stats;
            let proj = s.project_basename().unwrap_or("—").to_string();
            let model = model_display(s.model_raw());
            let name = e.name.unwrap_or("—").to_string();
            let mode = mode_display(s.permission_mode()).to_string();
            let ctx_cell = format!(
                "{} {:.0}%",
                fmt_tokens_k(s.last_context_size()),
                s.context_pct() * 100.0
            );
            let row_style = if idx == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(
                        format!("{} ", e.agent_status.dot()),
                        Style::default().fg(e.agent_status.color()),
                    ),
                    Span::styled(e.agent_status.label().to_string(), Style::default()),
                ])),
                Cell::from(proj),
                Cell::from(name),
                Cell::from(model),
                Cell::from(mode),
                Cell::from(format!("{}", s.assistant_messages())),
                Cell::from(ctx_cell),
                Cell::from(e.short_id.to_string()),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),  // status
            Constraint::Length(18), // project
            Constraint::Min(16),    // name (flex)
            Constraint::Length(18), // model
            Constraint::Length(7),  // mode — longest label "default"
            Constraint::Length(5),  // msgs
            Constraint::Length(9),  // ctx — `1.2M 100%` = 9 chars max
            Constraint::Length(10), // session
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(table, area);
}

/// Compact "k / M" formatter for token counts. Sub-1000 values render
/// untransformed so empty-session rows show `0` (not `0k`).
fn fmt_tokens_k(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Display the model name with the `claude-` prefix stripped. Empty / missing
/// model (no events yet) is rendered as `(waiting…)`.
fn model_display(raw: Option<&str>) -> String {
    let Some(name) = raw else {
        return "(waiting…)".to_string();
    };
    name.strip_prefix("claude-").unwrap_or(name).to_string()
}

/// Map the raw permission-mode string from JSONL to the short label shown
/// in the summary's `mode` column. Unknown / missing modes render as `—`.
fn mode_display(raw: Option<&str>) -> &'static str {
    match raw {
        Some("default") => "default",
        Some("plan") => "plan",
        Some("acceptEdits") => "edit",
        Some("bypassPermissions") => "auto",
        Some(_) | None => "—",
    }
}

fn draw_footer(f: &mut Frame<'_>, area: Rect) {
    let spans = vec![
        Span::raw(" "),
        Span::styled("q", bold()),
        Span::styled(" quit", dim()),
        Span::styled("  ·  ", dim()),
        Span::styled("↑↓", bold()),
        Span::styled(" select", dim()),
        Span::styled("  ·  ", dim()),
        Span::styled("Enter", bold()),
        Span::styled(" open", dim()),
        Span::styled("  ·  ", dim()),
        Span::styled("s", bold()),
        Span::styled(" single view", dim()),
        Span::styled("  ·  ", dim()),
        Span::styled("R", bold()),
        Span::styled(" rescan", dim()),
        Span::styled("  ·  ", dim()),
        Span::styled("b", bold()),
        Span::styled(" browser", dim()),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Coarse model family label used for both the aggregate breakdown and
/// the family color. Pattern-matches substrings against the raw model id
/// reported in JSONL.
fn model_family(raw: Option<&str>) -> String {
    let Some(s) = raw else {
        return "?".to_string();
    };
    let lower = s.to_lowercase();
    if lower.contains("opus") {
        "opus".to_string()
    } else if lower.contains("sonnet") {
        "sonnet".to_string()
    } else if lower.contains("haiku") {
        "haiku".to_string()
    } else {
        // Fall back to the first dash-segment so unknown families still render.
        lower.split('-').next().unwrap_or(&lower).to_string()
    }
}

fn family_color(family: &str) -> Color {
    match family {
        "opus" => Color::Magenta,
        "sonnet" => Color::Blue,
        "haiku" => Color::Cyan,
        _ => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::{AssistantEvent, AssistantMessage, ContentBlock, Event, Usage};
    use std::path::PathBuf;

    fn assistant_event(model: &str, usage: (u64, u64, u64, u64), ts: Option<&str>) -> Event {
        let (i, o, cw, cr) = usage;
        Event::Assistant(AssistantEvent {
            message: AssistantMessage {
                model: Some(model.into()),
                content: vec![ContentBlock::Other],
                usage: Some(Usage {
                    input_tokens: i,
                    output_tokens: o,
                    cache_creation_input_tokens: cw,
                    cache_read_input_tokens: cr,
                    cache_creation: None,
                }),
            },
            cwd: None,
            timestamp: ts.map(String::from),
        })
    }

    fn entry<'a>(
        stats: &'a SessionStats,
        agent_status: AgentStatus,
        name: Option<&'a str>,
        short_id: &'a str,
        path: &'a PathBuf,
    ) -> EntryView<'a> {
        EntryView {
            stats,
            agent_status,
            name,
            short_id,
            path,
        }
    }

    #[test]
    fn compute_aggregate_counts_live_and_offline_separately() {
        let mut a = SessionStats::default();
        let mut b = SessionStats::default();
        let mut c = SessionStats::default();
        a.ingest(&assistant_event("claude-opus-4-7", (10, 10, 0, 0), None));
        b.ingest(&assistant_event("claude-sonnet-4-6", (5, 5, 0, 0), None));
        c.ingest(&assistant_event("claude-opus-4-7", (5, 5, 0, 0), None));

        let p = PathBuf::from("/x.jsonl");
        let entries = vec![
            entry(&a, AgentStatus::Busy, Some("alpha"), "aa", &p),
            entry(&b, AgentStatus::Idle, None, "bb", &p),
            entry(&c, AgentStatus::Offline, Some("gamma"), "cc", &p),
        ];

        let agg = compute_aggregate(&entries);
        assert_eq!(agg.live_count, 2);
        assert_eq!(agg.total_msgs, 3);
        assert!(agg.total_cost_usd > 0.0);
        // BTreeMap order: opus, sonnet
        assert_eq!(
            agg.model_breakdown,
            vec![("opus".to_string(), 2), ("sonnet".to_string(), 1)]
        );
    }

    #[test]
    fn compute_aggregate_handles_empty_entries() {
        let agg = compute_aggregate(&[]);
        assert_eq!(agg.live_count, 0);
        assert_eq!(agg.total_msgs, 0);
        assert!((agg.total_cost_usd).abs() < f64::EPSILON);
        assert!(agg.model_breakdown.is_empty());
    }

    #[test]
    fn compute_aggregate_buckets_waiting_session_as_question_mark() {
        // Session with no ingested events → model_raw() is None.
        let s = SessionStats::default();
        let p = PathBuf::from("/x.jsonl");
        let entries = vec![entry(&s, AgentStatus::Busy, None, "aa", &p)];
        let agg = compute_aggregate(&entries);
        assert_eq!(agg.model_breakdown, vec![("?".to_string(), 1)]);
    }

    #[test]
    fn sort_order_puts_most_recent_first() {
        let mut a = SessionStats::default();
        let mut b = SessionStats::default();
        a.ingest(&assistant_event(
            "sonnet",
            (1, 1, 0, 0),
            Some("2026-04-27T11:00:00Z"),
        ));
        b.ingest(&assistant_event(
            "sonnet",
            (1, 1, 0, 0),
            Some("2026-04-27T12:00:00Z"),
        ));
        let p = PathBuf::from("/x.jsonl");
        let entries = vec![
            entry(&a, AgentStatus::Idle, None, "aa", &p), // older
            entry(&b, AgentStatus::Idle, None, "bb", &p), // newer
        ];
        let order = sort_order(&entries);
        assert_eq!(order, vec![1, 0], "newer event should come first");
    }

    #[test]
    fn sort_order_pushes_none_event_to_end() {
        let mut a = SessionStats::default();
        let b = SessionStats::default(); // no events
        a.ingest(&assistant_event(
            "sonnet",
            (1, 1, 0, 0),
            Some("2026-04-27T12:00:00Z"),
        ));
        let p = PathBuf::from("/x.jsonl");
        let entries = vec![
            entry(&b, AgentStatus::Offline, None, "aa", &p),
            entry(&a, AgentStatus::Busy, None, "bb", &p),
        ];
        let order = sort_order(&entries);
        assert_eq!(order, vec![1, 0], "event-bearing session comes first");
    }

    #[test]
    fn model_family_recognizes_known_families() {
        assert_eq!(model_family(Some("claude-opus-4-7")), "opus");
        assert_eq!(model_family(Some("claude-sonnet-4-6-1m")), "sonnet");
        assert_eq!(model_family(Some("claude-haiku-4-5")), "haiku");
        assert_eq!(model_family(None), "?");
    }

    #[test]
    fn fmt_tokens_k_buckets_sub_thousand_kilo_and_mega() {
        assert_eq!(fmt_tokens_k(0), "0");
        assert_eq!(fmt_tokens_k(999), "999");
        assert_eq!(fmt_tokens_k(1_000), "1k");
        assert_eq!(fmt_tokens_k(61_874), "61k");
        assert_eq!(fmt_tokens_k(999_999), "999k");
        assert_eq!(fmt_tokens_k(1_000_000), "1.0M");
        assert_eq!(fmt_tokens_k(1_234_567), "1.2M");
    }

    #[test]
    fn model_display_strips_claude_prefix() {
        assert_eq!(model_display(Some("claude-opus-4-7")), "opus-4-7");
        assert_eq!(model_display(Some("claude-sonnet-4-6")), "sonnet-4-6");
        assert_eq!(model_display(Some("claude-haiku-4-5")), "haiku-4-5");
        // Already-stripped names pass through unchanged.
        assert_eq!(model_display(Some("opus-4-7")), "opus-4-7");
        // Variant suffixes survive the strip.
        assert_eq!(model_display(Some("claude-opus-4-7[1m]")), "opus-4-7[1m]");
        // Waiting / unknown model.
        assert_eq!(model_display(None), "(waiting…)");
    }

    #[test]
    fn mode_display_maps_known_modes_and_falls_back() {
        assert_eq!(mode_display(Some("default")), "default");
        assert_eq!(mode_display(Some("plan")), "plan");
        assert_eq!(mode_display(Some("acceptEdits")), "edit");
        assert_eq!(mode_display(Some("bypassPermissions")), "auto");
        assert_eq!(mode_display(Some("unknownFutureMode")), "—");
        assert_eq!(mode_display(None), "—");
    }

    #[test]
    fn agent_status_dot_and_label() {
        assert_eq!(AgentStatus::Busy.dot(), "●");
        assert_eq!(AgentStatus::Idle.dot(), "●");
        assert_eq!(AgentStatus::Offline.dot(), "○");
        assert_eq!(AgentStatus::Busy.label(), "busy");
        assert_eq!(AgentStatus::Offline.label(), "off");
    }
}
