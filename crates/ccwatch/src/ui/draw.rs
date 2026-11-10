//! Frame rendering: the summary table (via `summary`) and the single-session
//! detail panels.

use super::SessionEntry;
use super::format::{fmt_int, format_duration, threshold_color, tool_color, value_span};
use super::state::{ViewMode, build_views, display_order};
use crate::stats::SessionStats;
use crate::style::{bold, bold_fg, dim};
use crate::summary;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Bar, BarChart, BarGroup, Block, Borders, Paragraph, Wrap},
};

// ---- Single-view layout heights (rows). ----
const HEADER_H: u16 = 3;
const CTX_GAUGE_H: u16 = 3;
const TOKENS_H: u16 = 4;
const COST_H: u16 = 3;
const TOOLS_H: u16 = 3;
const SPARKLINE_MIN_H: u16 = 3;
const FOOTER_H: u16 = 1;

pub(super) fn draw_frame(
    f: &mut Frame<'_>,
    entries: &[SessionEntry],
    selected: usize,
    mode: ViewMode,
) {
    match mode {
        ViewMode::Summary => {
            let views = build_views(entries);
            summary::draw(f, f.area(), &views, selected);
        }
        ViewMode::Single => {
            if let Some(entry) = entries.get(selected) {
                // Show the 1-based position in the *display* order (same order
                // Tab/n walks), so the `[n/total]` counter stays monotonic.
                let order = display_order(entries);
                let pos = order.iter().position(|&i| i == selected).unwrap_or(0);
                draw_single(f, entry, pos, entries.len());
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
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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
