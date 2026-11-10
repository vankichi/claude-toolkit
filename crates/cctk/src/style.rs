//! Text styles and the bordered panel shared by the tools' ratatui views.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders};

/// A block with a full border and `title`; chain `.border_style(..)` etc. for
/// variants.
#[must_use]
pub fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::default().borders(Borders::ALL).title(title)
}

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

/// Black bold text on green: the selected row of a list.
#[must_use]
pub fn highlight() -> Style {
    Style::default()
        .bg(Color::Green)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD)
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
    fn panel_is_a_titled_full_border() {
        assert_eq!(Block::bordered(), Block::default().borders(Borders::ALL));
        assert_eq!(
            panel(" t "),
            Block::default().borders(Borders::ALL).title(" t ")
        );
        assert_eq!(
            panel(String::from("t")).border_style(dim()),
            Block::default()
                .borders(Borders::ALL)
                .border_style(dim())
                .title("t")
        );
    }

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
        assert_eq!(
            highlight(),
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        );
    }
}
