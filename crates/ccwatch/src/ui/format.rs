//! Span / color / number helpers (shared styles live in `style`).

use crate::style::bold_fg;
use ratatui::{style::Color, text::Span};

pub(super) fn value_span<S: Into<String>>(s: S, color: Color) -> Span<'static> {
    Span::styled(s.into(), bold_fg(color))
}

#[must_use]
pub(super) fn threshold_color(value: f64, warn: f64, crit: f64) -> Color {
    if value < warn {
        Color::Green
    } else if value < crit {
        Color::Yellow
    } else {
        Color::Red
    }
}

#[must_use]
pub(super) fn fmt_int(n: u64) -> String {
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
pub(super) fn format_duration(secs: u64) -> String {
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
pub(super) fn tool_color(name: &str) -> Color {
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
