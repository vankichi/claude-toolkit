//! Rendering: the frame layout, the trends band, the list/detail body, the
//! full-screen graph and the project picker popup.

use super::AppState;
use super::format::{
    PALETTE, detail_lines, format_recency, model_color, provenance_color, source_label,
};
use crate::model::{Category, ProjectFilter};
use crate::usage::{GraphMetric, TREND_DAYS};
use chrono::{Duration, NaiveDate};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};

const TABS_H: u16 = 3;
const RUN_H: u16 = 1;
const HINT_H: u16 = 1;
const LIST_PCT: u16 = 45;
const DETAIL_PCT: u16 = 55;
/// Height of the always-on top trends graph band.
const TRENDS_H: u16 = 9;
/// How many series the trends graph overlays.
const TRENDS_TOP_N: usize = 5;

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
        // Always-on top trends graph, then the list + detail beneath it.
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(TRENDS_H), Constraint::Min(0)])
            .split(body);
        draw_trends(f, split[0], state);
        draw_body(f, split[1], state);
    }
    draw_hint(f, hint, state);

    if state.picking {
        draw_project_picker(f, f.area(), state);
    }
}

/// The top-N colored trend series for the current tab/window/sort/filter and
/// selected metric. Public so a host (e.g. `cctop`'s overview) can render the
/// same graph. Model rows are colored by family; other tabs cycle a palette.
#[must_use]
pub fn trends_series(state: &AppState) -> Vec<cctk::chart::LineSeries> {
    let rows = state.rows();
    let span = state.graph_span();
    rows.iter()
        .take(TRENDS_TOP_N)
        .enumerate()
        .map(|(i, r)| {
            let color = match state.tab {
                Category::Model => model_color(&rows, &r.name),
                _ => PALETTE[i % PALETTE.len()],
            };
            let points: Vec<(f64, f64)> = state
                .graph_series(&r.name, span)
                .into_iter()
                .enumerate()
                .map(|(x, y)| (x as f64, y))
                .collect();
            cctk::chart::LineSeries {
                points,
                label: r.name.clone(),
                color,
            }
        })
        .collect()
}

/// Always-on top band: an overlaid braille line chart of the top series'
/// daily trend for the current metric, with a colored legend and a date
/// x-axis. Model rows are colored by family; other tabs cycle a palette.
fn draw_trends(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let metric = state.graph_metric;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(
            " Trends · {} · top {} · window {} · (m: metric) ",
            metric.label(),
            TRENDS_TOP_N,
            state.window.label(),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let series = trends_series(state);
    if series.is_empty() || inner.height < 2 {
        f.render_widget(
            Paragraph::new(Span::styled(
                "no data in the selected window",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    // Manual colored legend (ratatui's auto-legend hides at this size) + chart.
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let legend: Vec<Span> = series
        .iter()
        .map(|s| {
            let name: String = s.label.chars().take(16).collect();
            Span::styled(format!("▉ {name}   "), Style::default().fg(s.color))
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(legend)), parts[0]);

    let y_max = series
        .iter()
        .flat_map(|s| s.points.iter().map(|&(_, y)| y))
        .fold(0.0_f64, f64::max)
        .max(1.0);

    let chart = cctk::chart::braille_multi_line(&series, y_max, metric_top_label(metric, y_max))
        .x_axis(date_axis(state.today_for_rescan(), state.graph_span()));
    f.render_widget(chart, parts[1]);
}

/// X axis for a `span`-day series ending `today`, labeled with its first and
/// last dates.
fn date_axis(today: NaiveDate, span: usize) -> Axis<'static> {
    #[allow(clippy::cast_possible_wrap)] // span is a small day count
    let oldest = (today - Duration::days(span as i64 - 1))
        .format("%m/%d")
        .to_string();
    let newest = today.format("%m/%d").to_string();
    let last_x = (span.saturating_sub(1)).max(1) as f64;
    cctk::chart::x_axis(last_x, oldest, newest)
}

/// Y-axis top label for the trends graph, formatted for the metric.
fn metric_top_label(metric: GraphMetric, y_max: f64) -> String {
    if metric == GraphMetric::Cost {
        return format!("${y_max:.2}");
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    cctk::fmt::tokens(y_max.round() as u64)
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

/// Full-screen bottom-style braille line chart of the selected row's trend
/// over the current window (7d / 30d / all-time) for the selected metric.
fn draw_graph(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let Some(row) = state.selected_row() else {
        return;
    };
    let today = state.today_for_rescan();
    let span = state.graph_span();

    let values = state.graph_series(&row.name, span);
    let points: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v))
        .collect();
    let peak = values.iter().copied().fold(0.0_f64, f64::max).max(1.0);

    let title = format!(
        " {}: {} · {} in {} · {} trend · last {} (g/Esc close) ",
        state.tab.title(),
        row.name,
        row.count,
        state.window.label(),
        state.graph_metric.label(),
        format_recency(row.last_used, today),
    );

    let chart = cctk::chart::braille_line(
        &points,
        peak,
        metric_top_label(state.graph_metric, peak),
        Color::Green,
    )
    .block(Block::default().borders(Borders::ALL).title(title))
    .x_axis(date_axis(today, span));
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
                // Match the trends graph: a stable distinct color per model
                // (separates variants and unknown families, unlike family hues).
                Category::Model => model_color(&rows, &r.name),
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
        " q quit · Tab tabs · j/k move · w window · s sort · p project · m metric · g graph · / filter · R rescan "
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
    use crate::live::ActiveSet;
    use crate::model::Window;
    use crate::provenance::ProvenanceMap;
    use crate::ui::test_support::{day, db_with};

    /// `draw(st)` on an 80×12 test terminal, as one string of cell symbols.
    fn render(st: &AppState) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|f| draw(f, st)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
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
    fn watch_mode_renders_running_line_and_active_spinner() {
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

        let grid = render(&st);

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
        let db = db_with(&[(Category::Skill, "brainstorm", "p")]);
        let mut st = AppState::new(db, ProvenanceMap::default(), day(2026, 7, 16));
        st.tab = Category::Skill;
        st.window = Window::All;
        // watch stays false.

        let grid = render(&st);

        assert!(!grid.contains("running:"));
        assert!(!grid.contains("· watch"));
    }
}
