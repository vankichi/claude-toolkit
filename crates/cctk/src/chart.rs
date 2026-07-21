//! Bottom-style braille line-chart builder (ratatui widget helper).
//!
//! Returns a configured [`ratatui::widgets::Chart`] plotting `points` as a
//! connected braille line with a `0..y_max` y-axis. The caller owns `points`
//! (the chart borrows them) and may further customize the returned chart —
//! e.g. `.block(..)` for a border/title or `.x_axis(..)` to add time labels.

use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::Line;
use ratatui::widgets::{Axis, Chart, Dataset, GraphType};

/// Build a braille line chart over `points` (already in `(x, y)` form), scaled
/// to `[0, y_max]`, with `"0"` and `y_top_label` on the y-axis and `color` for
/// the line. The x-axis spans `[0, last x]` with no labels; override it on the
/// returned chart if you want tick labels.
#[must_use]
pub fn braille_line(
    points: &[(f64, f64)],
    y_max: f64,
    y_top_label: impl Into<String>,
    color: Color,
) -> Chart<'_> {
    let last_x = points.last().map_or(1.0, |&(x, _)| x).max(1.0);
    let dataset = Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color))
        .data(points);
    Chart::new(vec![dataset])
        .x_axis(Axis::default().bounds([0.0, last_x]))
        .y_axis(
            Axis::default()
                .bounds([0.0, y_max.max(1.0)])
                .labels([Line::from("0"), Line::from(y_top_label.into())])
                .style(Style::default().fg(Color::DarkGray)),
        )
}

/// One colored series for [`braille_multi_line`]. Owns its points so the
/// caller can build a `Vec<LineSeries>` and pass it by reference.
pub struct LineSeries {
    pub points: Vec<(f64, f64)>,
    pub label: String,
    pub color: Color,
}

/// Build an overlaid multi-series braille line chart sharing one `[0, y_max]`
/// y-axis. Each series draws in its own color and contributes its label to the
/// auto-rendered legend. The x-axis spans `[0, max last x]` with no labels.
#[must_use]
pub fn braille_multi_line(
    series: &[LineSeries],
    y_max: f64,
    y_top_label: impl Into<String>,
) -> Chart<'_> {
    let last_x = series
        .iter()
        .filter_map(|s| s.points.last())
        .map(|&(x, _)| x)
        .fold(1.0_f64, f64::max);
    let datasets: Vec<Dataset> = series
        .iter()
        .map(|s| {
            Dataset::default()
                .name(Line::from(s.label.clone()).style(Style::default().fg(s.color)))
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(s.color))
                .data(&s.points)
        })
        .collect();
    Chart::new(datasets)
        .x_axis(Axis::default().bounds([0.0, last_x]))
        .y_axis(
            Axis::default()
                .bounds([0.0, y_max.max(1.0)])
                .labels([Line::from("0"), Line::from(y_top_label.into())])
                .style(Style::default().fg(Color::DarkGray)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(w: u16, h: u16, points: &[(f64, f64)], y_max: f64) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| {
                let chart = braille_line(points, y_max, "max", Color::Cyan);
                f.render_widget(chart, f.area());
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
        }
        out
    }

    #[test]
    fn plots_braille_glyphs() {
        let points: Vec<(f64, f64)> = (0..20).map(|i| (f64::from(i), f64::from(i % 5))).collect();
        let out = render(40, 6, &points, 4.0);
        assert!(
            out.chars()
                .any(|ch| ('\u{2800}'..='\u{28FF}').contains(&ch)),
            "expected braille glyphs in the line chart"
        );
    }

    #[test]
    fn shows_y_axis_labels() {
        let points = [(0.0, 0.0), (1.0, 1.0)];
        let out = render(30, 5, &points, 1.0);
        // The custom top label and the "0" baseline should render.
        assert!(out.contains("max"));
        assert!(out.contains('0'));
    }

    #[test]
    fn multi_line_plots_all_series() {
        let series = vec![
            LineSeries {
                points: (0..10).map(|i| (f64::from(i), f64::from(i))).collect(),
                label: "opus".into(),
                color: Color::Magenta,
            },
            LineSeries {
                points: (0..10).map(|i| (f64::from(i), f64::from(9 - i))).collect(),
                label: "haiku".into(),
                color: Color::Cyan,
            },
        ];
        let mut terminal = Terminal::new(TestBackend::new(50, 8)).unwrap();
        terminal
            .draw(|f| {
                let chart = braille_multi_line(&series, 9.0, "9");
                f.render_widget(chart, f.area());
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..8 {
            for x in 0..50 {
                out.push_str(buf[(x, y)].symbol());
            }
        }
        assert!(
            out.chars()
                .any(|ch| ('\u{2800}'..='\u{28FF}').contains(&ch)),
            "expected braille glyphs from the overlaid series"
        );
    }
}
