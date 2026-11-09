//! Text styles shared by the summary and single-session views.

use ratatui::style::{Color, Modifier, Style};

pub(crate) fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub(crate) fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

pub(crate) fn bold_fg(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}
