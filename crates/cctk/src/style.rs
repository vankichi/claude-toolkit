//! Text styles shared by the tools' ratatui views.

use ratatui::style::{Color, Modifier, Style};

/// Dim (dark gray) text for secondary information.
#[must_use]
pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Bold text in the default color.
#[must_use]
pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Bold text in color `c`.
#[must_use]
pub fn bold_fg(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}

/// Reverse-video, used to mark the selected row.
#[must_use]
pub fn reversed() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_match_their_inline_forms() {
        assert_eq!(dim(), Style::default().fg(Color::DarkGray));
        assert_eq!(bold(), Style::default().add_modifier(Modifier::BOLD));
        assert_eq!(
            bold_fg(Color::Cyan),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(
            reversed(),
            Style::default().add_modifier(Modifier::REVERSED)
        );
    }
}
