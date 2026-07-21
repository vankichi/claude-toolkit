//! Overview rendering: top "Now" band, a Top-usage / Config-map two-column
//! middle, and a Recent-tools bottom band. Usage magnitudes render as braille
//! dot bars (`cctk::viz`); the token-rate trend is a bottom-style braille line
//! chart (ratatui `Chart`). The selected panel gets a highlighted border.
//!
//! The `draw*` functions are ratatui glue (not unit-tested); the numeric
//! formatting helpers are pure and covered by tests.

use crate::app::{App, Mode, Panel};
use crate::store::Dashboard;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Paragraph},
};

/// Human-readable token count: `1.2M`, `900k`, or the bare number.
#[must_use]
pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Dollar cost with cents.
#[must_use]
pub fn fmt_cost(cost: f64) -> String {
    format!("${cost:.2}")
}

fn panel_block(title: &str, panel: Panel, app: &App) -> Block<'static> {
    let selected = app.mode == Mode::Overview && app.selected == panel;
    let border = if selected {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(format!(" {title} "))
}

/// Render the whole overview into `f`.
pub fn draw(f: &mut Frame<'_>, dash: &Dashboard, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(8),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .split(area);
    draw_now(f, rows[0], dash, app);

    let cols =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(rows[1]);
    draw_usage(f, cols[0], dash, app);
    draw_map(f, cols[1], dash, app);

    draw_recent(f, rows[2], dash, app);
}

fn draw_now(f: &mut Frame<'_>, area: Rect, dash: &Dashboard, app: &App) {
    let now = &dash.now;
    let block = panel_block("Now: active session  [1]", Panel::Now, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Two header rows (model/tokens/cost + context gauge) then the rate chart.
    let parts = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(inner);
    let header = parts[0];
    let chart_area = parts[1];

    let ctx_w = usize::from(header.width.saturating_sub(28)).clamp(6, 40);
    let ctx_bar = cctk::viz::dot_bar(now.context_pct(), ctx_w);
    let pct = now.context_pct() * 100.0;
    let head = Line::from(vec![
        Span::styled(
            now.model_label().to_string(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "   {} tok   {}",
            fmt_tokens(now.total_tokens()),
            fmt_cost(now.cost_usd())
        )),
    ]);
    let ctx_line = Line::from(format!("ctx {ctx_bar} {pct:.0}%"));
    f.render_widget(Paragraph::new(vec![head, ctx_line]), header);

    let points = now.rate_points(chrono::Utc::now(), RATE_WINDOW_SECS, RATE_BINS);
    draw_rate_chart(f, chart_area, &points);
}

/// Wall-clock window (seconds) the Now rate chart covers.
const RATE_WINDOW_SECS: i64 = 900;
/// Time buckets across the rate window (finer = smoother line).
const RATE_BINS: usize = 30;
/// Left x-axis label for the rate window.
const RATE_LEFT_LABEL: &str = "-15m";

/// A bottom-style braille line chart of the real-time tokens/minute rate.
/// `points` are `(bin, tokens_per_minute)` spanning `[now - window, now]`.
/// Falls back to a hint while the window holds no activity.
fn draw_rate_chart(f: &mut Frame<'_>, area: Rect, points: &[(f64, f64)]) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let peak = points.iter().map(|&(_, y)| y).fold(0.0_f64, f64::max);
    if points.len() < 2 || peak <= 0.0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "token rate — waiting for activity…",
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }

    // `peak` is tokens/minute (from non-negative token counts), so the
    // round-trip cast for the axis label cannot truncate or lose a sign.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let peak_label = format!("{}/m", fmt_tokens(peak.round() as u64));
    let last_x = points.last().map_or(1.0, |&(x, _)| x).max(1.0);

    let chart = cctk::chart::braille_line(points, peak, peak_label, Color::Cyan).x_axis(
        Axis::default()
            .bounds([0.0, last_x])
            .labels([Line::from(RATE_LEFT_LABEL), Line::from("now")])
            .style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(chart, area);
}

fn draw_usage(f: &mut Frame<'_>, area: Rect, dash: &Dashboard, app: &App) {
    let metric = dash.stats.graph_metric;
    let title = format!("Top usage  [2] · {} (m)", metric.label());
    let block = panel_block(&title, Panel::Stats, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Overlaid per-series trend graph (shared with ccstat), colored by family.
    let series = ccstat::ui::trends_series(&dash.stats);
    if series.is_empty() || inner.height < 2 {
        f.render_widget(
            Paragraph::new(Span::styled(
                "no usage recorded",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    let parts = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
    let legend: Vec<Span> = series
        .iter()
        .map(|s| {
            let name: String = s.label.chars().take(12).collect();
            Span::styled(format!("▉{name} "), Style::default().fg(s.color))
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(legend)), parts[0]);

    let y_max = series
        .iter()
        .flat_map(|s| s.points.iter().map(|&(_, y)| y))
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let y_label = if metric == ccstat::usage::GraphMetric::Cost {
        fmt_cost(y_max)
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = y_max.round() as u64;
        fmt_tokens(v)
    };

    let today = dash.stats.today_for_rescan();
    #[allow(clippy::cast_possible_wrap)] // TREND_DAYS is a small constant
    let offset = ccstat::usage::TREND_DAYS as i64 - 1;
    let oldest = (today - chrono::Duration::days(offset))
        .format("%m/%d")
        .to_string();
    let newest = today.format("%m/%d").to_string();
    let last_x = ccstat::usage::TREND_DAYS.saturating_sub(1).max(1) as f64;

    let chart = cctk::chart::braille_multi_line(&series, y_max, y_label).x_axis(
        Axis::default()
            .bounds([0.0, last_x])
            .labels([Line::from(oldest), Line::from(newest)])
            .style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(chart, parts[1]);
}

fn draw_map(f: &mut Frame<'_>, area: Rect, dash: &Dashboard, app: &App) {
    let block = panel_block("Config map  [3]", Panel::Map, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let c = &dash.map_counts;
    let row = |label: &str, n: usize| Line::from(format!("{label:<10} {n:>4}"));
    let lines = vec![
        row("agents", c.agents),
        row("skills", c.skills),
        row("commands", c.commands),
        row("plugins", c.plugins),
        row("mcp", c.mcp),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_recent(f: &mut Frame<'_>, area: Rect, dash: &Dashboard, app: &App) {
    let title = if app.filtering || !app.filter.is_empty() {
        format!("Recent tools   /{}", app.filter)
    } else {
        "Recent tools   ( / filter · 1/2/3 select · Enter drill · q quit )".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" {title} "));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let tools: Vec<String> = dash.now.recent_tools().map(str::to_string).collect();
    let text = if tools.is_empty() {
        "—".to_string()
    } else {
        tools.join("  ")
    };
    f.render_widget(Paragraph::new(Line::from(text)), inner);
}

/// Full-screen "Now" detail shown when the Now panel is drilled into.
pub fn draw_now_detail(f: &mut Frame<'_>, now: &crate::now::NowStats) {
    let area = f.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Now — active session   (Esc/q to return) ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Text summary on top, then a large token-rate line chart filling the rest.
    let parts = Layout::vertical([Constraint::Length(8), Constraint::Min(0)]).split(inner);
    let text_area = parts[0];
    let chart_area = parts[1];

    let ctx_w = usize::from(text_area.width.saturating_sub(28)).clamp(6, 60);
    let ctx_bar = cctk::viz::dot_bar(now.context_pct(), ctx_w);

    let lines = vec![
        Line::from(vec![
            Span::raw("model    "),
            Span::styled(
                now.model_label().to_string(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(format!("tokens   {}", fmt_tokens(now.total_tokens()))),
        Line::from(format!("cost     {}", fmt_cost(now.cost_usd()))),
        Line::from(format!("messages {}", now.assistant_messages())),
        Line::from(format!(
            "context  {ctx_bar} {:.0}%  ({} / {})",
            now.context_pct() * 100.0,
            fmt_tokens(now.last_context_size()),
            fmt_tokens(now.context_window()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "token rate (tok/msg):",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "recent tools: {}",
                now.recent_tools().collect::<Vec<_>>().join("  ")
            ),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(lines), text_area);
    let points = now.rate_points(chrono::Utc::now(), RATE_WINDOW_SECS, RATE_BINS);
    draw_rate_chart(f, chart_area, &points);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_to_string(w: u16, h: u16, points: &[(f64, f64)]) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| draw_rate_chart(f, f.area(), points))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn rate_chart_renders_braille_line() {
        let points: Vec<(f64, f64)> = (0..24)
            .map(|i| (f64::from(i), ((f64::from(i) * 0.5).sin() + 1.0) * 1000.0))
            .collect();
        let out = render_to_string(48, 6, &points);
        assert!(
            out.chars()
                .any(|ch| ('\u{2800}'..='\u{28FF}').contains(&ch)),
            "expected braille glyphs in the rate chart"
        );
    }

    #[test]
    fn rate_chart_hint_when_idle() {
        // All-zero rate (no activity) shows the waiting hint.
        let points = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)];
        let out = render_to_string(48, 4, &points);
        assert!(out.contains("waiting"));
    }

    #[test]
    fn fmt_tokens_scales() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1_000), "1k");
        assert_eq!(fmt_tokens(12_800), "13k");
        assert_eq!(fmt_tokens(1_200_000), "1.2M");
    }

    #[test]
    fn fmt_cost_has_cents() {
        assert_eq!(fmt_cost(0.0), "$0.00");
        assert_eq!(fmt_cost(3.4), "$3.40");
        assert_eq!(fmt_cost(142.5), "$142.50");
    }
}
