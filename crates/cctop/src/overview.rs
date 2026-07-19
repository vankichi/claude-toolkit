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
    widgets::{Block, Borders, Paragraph},
};

/// How many usage rows the compact Top-usage panel shows.
const TOP_USAGE_ROWS: usize = 8;

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

    draw_rate_chart(f, chart_area, now.rate_series());
}

/// A bottom-style braille line chart of the token-rate series into `area`.
/// Falls back to a hint until at least two samples exist (a line needs two
/// points).
fn draw_rate_chart(f: &mut Frame<'_>, area: Rect, series: &[f64]) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    if series.len() < 2 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "token rate — waiting for activity…",
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }

    let points: Vec<(f64, f64)> = series
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v))
        .collect();
    let peak = series.iter().copied().fold(0.0_f64, f64::max).max(1.0);

    // `peak` is derived from u64 token counts (non-negative, small), so the
    // round-trip cast for the axis label cannot truncate or lose a sign.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let peak_label = fmt_tokens(peak.round() as u64);

    let chart = cctk::chart::braille_line(&points, peak, peak_label, Color::Cyan);
    f.render_widget(chart, area);
}

fn draw_usage(f: &mut Frame<'_>, area: Rect, dash: &Dashboard, app: &App) {
    let block = panel_block("Top usage  [2]", Panel::Stats, app);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = dash.stats.rows();
    let max_cost = rows
        .iter()
        .map(|r| r.cost_usd)
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    let name_w = 16usize;
    let bar_w = usize::from(inner.width)
        .saturating_sub(name_w + 10)
        .clamp(4, 24);

    let lines: Vec<Line> = rows
        .iter()
        .take(TOP_USAGE_ROWS)
        .map(|r| {
            let bar = cctk::viz::dot_bar(r.cost_usd / max_cost, bar_w);
            let name: String = r.name.chars().take(name_w).collect();
            Line::from(format!(
                "{name:<name_w$} {cost:>7} {bar}",
                cost = fmt_cost(r.cost_usd),
            ))
        })
        .collect();

    let body = if lines.is_empty() {
        vec![Line::from(Span::styled(
            "no usage recorded",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        lines
    };
    f.render_widget(Paragraph::new(body), inner);
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
    draw_rate_chart(f, chart_area, now.rate_series());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_to_string(w: u16, h: u16, series: &[f64]) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| draw_rate_chart(f, f.area(), series))
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
        let series: Vec<f64> = (0..24)
            .map(|i| ((f64::from(i) * 0.5).sin() + 1.0) * 1000.0)
            .collect();
        let out = render_to_string(48, 6, &series);
        assert!(
            out.chars()
                .any(|ch| ('\u{2800}'..='\u{28FF}').contains(&ch)),
            "expected braille glyphs in the rate chart"
        );
    }

    #[test]
    fn rate_chart_hint_when_too_few_samples() {
        let out = render_to_string(48, 4, &[5.0]);
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
