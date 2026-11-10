//! The terminal around each tool's TUI: entering the alternate screen and
//! raw mode, leaving them again, and reading key presses.

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind};
use std::ops::{Deref, DerefMut};

/// The TUI's terminal, restored when dropped: on return, on `?` and on
/// unwinding. Drop it explicitly where work must run after the restore.
pub struct Tui(DefaultTerminal);

impl Tui {
    /// Enters the alternate screen and raw mode. A failure part-way leaves
    /// the terminal restored.
    ///
    /// # Errors
    /// When the terminal cannot be set up.
    pub fn init() -> std::io::Result<Self> {
        ratatui::try_init()
            .inspect_err(|_| ratatui::restore())
            .map(Self)
    }
}

impl Deref for Tui {
    type Target = DefaultTerminal;
    fn deref(&self) -> &DefaultTerminal {
        &self.0
    }
}

impl DerefMut for Tui {
    fn deref_mut(&mut self) -> &mut DefaultTerminal {
        &mut self.0
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// The key of a key press. Releases and repeats (which some terminals
/// report as separate events) and non-key events are `None`.
#[must_use]
pub fn pressed(event: &Event) -> Option<KeyCode> {
    match event {
        Event::Key(k) if k.kind == KeyEventKind::Press => Some(k.code),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEvent, KeyEventState, KeyModifiers};

    fn key(kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
            kind,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn only_a_press_yields_its_key() {
        assert_eq!(pressed(&key(KeyEventKind::Press)), Some(KeyCode::Char('q')));
        assert_eq!(pressed(&key(KeyEventKind::Release)), None);
        assert_eq!(pressed(&key(KeyEventKind::Repeat)), None);
        assert_eq!(pressed(&Event::FocusGained), None);
        assert_eq!(pressed(&Event::Resize(80, 24)), None);
    }
}
