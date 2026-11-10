//! Key classification: pure `KeyCode` → semantic action mapping.

use super::state::ViewMode;
use crossterm::event::KeyCode;

/// Decoded keyboard intent. Pure mapping from `KeyCode` → semantic action,
/// extracted so it can be unit-tested without spinning up a terminal.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum KeyAction {
    Quit,
    ResetCurrent,
    OpenBrowser,
    Rescan,
    SelectPrev,
    SelectNext,
    OpenSelected, // Summary → Single on current selection
    ToggleView,   // s key
    NextInSingle, // Tab / n in single mode
    PrevInSingle, // BackTab / p in single mode
    Ignore,
}

/// Map a keypress to a semantic action. The mode (and pinned-ness) gates
/// which actions are reachable.
pub(super) fn classify_key(code: KeyCode, mode: ViewMode, pinned: bool) -> KeyAction {
    match (mode, code) {
        (_, KeyCode::Char('q') | KeyCode::Esc) => KeyAction::Quit,
        (_, KeyCode::Char('b')) => KeyAction::OpenBrowser,
        (ViewMode::Summary, KeyCode::Up | KeyCode::Char('k')) => KeyAction::SelectPrev,
        (ViewMode::Summary, KeyCode::Down | KeyCode::Char('j')) => KeyAction::SelectNext,
        (ViewMode::Summary, KeyCode::Enter | KeyCode::Char('s')) => KeyAction::OpenSelected,
        (ViewMode::Summary, KeyCode::Char('R')) => KeyAction::Rescan,
        (ViewMode::Single, KeyCode::Char('s')) if !pinned => KeyAction::ToggleView,
        (ViewMode::Single, KeyCode::Char('r')) => KeyAction::ResetCurrent,
        (ViewMode::Single, KeyCode::Char('R')) if !pinned => KeyAction::Rescan,
        (ViewMode::Single, KeyCode::Tab | KeyCode::Char('n')) if !pinned => KeyAction::NextInSingle,
        (ViewMode::Single, KeyCode::BackTab | KeyCode::Char('p')) if !pinned => {
            KeyAction::PrevInSingle
        }
        _ => KeyAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_key_quit_always_works() {
        assert_eq!(
            classify_key(KeyCode::Char('q'), ViewMode::Summary, false),
            KeyAction::Quit
        );
        assert_eq!(
            classify_key(KeyCode::Esc, ViewMode::Single, true),
            KeyAction::Quit
        );
    }

    #[test]
    fn classify_key_summary_select_arrows_and_vim() {
        assert_eq!(
            classify_key(KeyCode::Up, ViewMode::Summary, false),
            KeyAction::SelectPrev
        );
        assert_eq!(
            classify_key(KeyCode::Char('k'), ViewMode::Summary, false),
            KeyAction::SelectPrev
        );
        assert_eq!(
            classify_key(KeyCode::Down, ViewMode::Summary, false),
            KeyAction::SelectNext
        );
        assert_eq!(
            classify_key(KeyCode::Char('j'), ViewMode::Summary, false),
            KeyAction::SelectNext
        );
    }

    #[test]
    fn classify_key_open_selected_via_enter_or_s() {
        assert_eq!(
            classify_key(KeyCode::Enter, ViewMode::Summary, false),
            KeyAction::OpenSelected
        );
        assert_eq!(
            classify_key(KeyCode::Char('s'), ViewMode::Summary, false),
            KeyAction::OpenSelected
        );
    }

    #[test]
    fn classify_key_single_s_toggles_unless_pinned() {
        assert_eq!(
            classify_key(KeyCode::Char('s'), ViewMode::Single, false),
            KeyAction::ToggleView
        );
        assert_eq!(
            classify_key(KeyCode::Char('s'), ViewMode::Single, true),
            KeyAction::Ignore
        );
    }

    #[test]
    fn classify_key_single_nav_requires_unpinned() {
        assert_eq!(
            classify_key(KeyCode::Tab, ViewMode::Single, false),
            KeyAction::NextInSingle
        );
        assert_eq!(
            classify_key(KeyCode::Tab, ViewMode::Single, true),
            KeyAction::Ignore
        );
        assert_eq!(
            classify_key(KeyCode::BackTab, ViewMode::Single, false),
            KeyAction::PrevInSingle
        );
    }

    #[test]
    fn classify_key_reset_only_in_single() {
        assert_eq!(
            classify_key(KeyCode::Char('r'), ViewMode::Single, false),
            KeyAction::ResetCurrent
        );
        // 'r' in summary mode is unbound — selection refresh isn't a thing.
        assert_eq!(
            classify_key(KeyCode::Char('r'), ViewMode::Summary, false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn classify_key_rescan_blocked_when_pinned() {
        assert_eq!(
            classify_key(KeyCode::Char('R'), ViewMode::Summary, false),
            KeyAction::Rescan
        );
        assert_eq!(
            classify_key(KeyCode::Char('R'), ViewMode::Single, false),
            KeyAction::Rescan
        );
        assert_eq!(
            classify_key(KeyCode::Char('R'), ViewMode::Single, true),
            KeyAction::Ignore
        );
    }
}
