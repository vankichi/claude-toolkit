//! Key classification (semantic intent) and the per-keypress state update.

use super::{AppState, rescan};
use crate::scan::ScanConfig;
use crossterm::event::KeyCode;

/// Semantic key intent. `filtering` and `picking` are modal: they capture
/// keys before the normal navigation bindings.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    Quit,
    NextTab,
    PrevTab,
    Next,
    Prev,
    CycleWindow,
    CycleSort,
    OpenProjectPicker,
    StartFilter,
    FilterChar(char),
    FilterBackspace,
    FilterClear,
    FilterConfirm,
    PickerNext,
    PickerPrev,
    PickerApply,
    PickerCancel,
    Rescan,
    ToggleGraph,
    CycleMetric,
    Ignore,
}

#[must_use]
pub fn classify_key(
    code: KeyCode,
    filtering: bool,
    picking: bool,
    showing_graph: bool,
) -> KeyAction {
    if showing_graph {
        return match code {
            KeyCode::Char('g') | KeyCode::Esc => KeyAction::ToggleGraph,
            KeyCode::Char('q') => KeyAction::Quit,
            _ => KeyAction::Ignore,
        };
    }
    if picking {
        return match code {
            KeyCode::Char('j') | KeyCode::Down => KeyAction::PickerNext,
            KeyCode::Char('k') | KeyCode::Up => KeyAction::PickerPrev,
            KeyCode::Enter => KeyAction::PickerApply,
            KeyCode::Esc => KeyAction::PickerCancel,
            _ => KeyAction::Ignore,
        };
    }
    if filtering {
        return match code {
            KeyCode::Esc => KeyAction::FilterClear,
            KeyCode::Enter => KeyAction::FilterConfirm,
            KeyCode::Backspace => KeyAction::FilterBackspace,
            KeyCode::Char(c) => KeyAction::FilterChar(c),
            _ => KeyAction::Ignore,
        };
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => KeyAction::Quit,
        KeyCode::Tab => KeyAction::NextTab,
        KeyCode::BackTab => KeyAction::PrevTab,
        KeyCode::Char('j') | KeyCode::Down => KeyAction::Next,
        KeyCode::Char('k') | KeyCode::Up => KeyAction::Prev,
        KeyCode::Char('w') => KeyAction::CycleWindow,
        KeyCode::Char('s') => KeyAction::CycleSort,
        KeyCode::Char('p') => KeyAction::OpenProjectPicker,
        KeyCode::Char('/') => KeyAction::StartFilter,
        KeyCode::Char('R') => KeyAction::Rescan,
        KeyCode::Char('g') => KeyAction::ToggleGraph,
        KeyCode::Char('m') => KeyAction::CycleMetric,
        _ => KeyAction::Ignore,
    }
}

/// Applies one keypress. Returns `true` when the user wants to quit.
///
/// Part of the re-entrant view API: a host (e.g. `cctop` drill-down) can own
/// the terminal and event loop, calling [`draw`](super::draw) each frame and
/// `handle_key` per keypress. A `true` return means "leave this view" (back to
/// the host).
pub fn handle_key(
    code: KeyCode,
    state: &mut AppState,
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
) -> bool {
    match classify_key(code, state.filtering, state.picking, state.showing_graph) {
        KeyAction::Quit => return true,
        KeyAction::NextTab => state.next_tab(),
        KeyAction::PrevTab => state.prev_tab(),
        KeyAction::Next => state.next(),
        KeyAction::Prev => state.prev(),
        KeyAction::CycleWindow => state.cycle_window(),
        KeyAction::CycleSort => state.cycle_sort(),
        KeyAction::OpenProjectPicker => state.open_project_picker(),
        KeyAction::StartFilter => state.filtering = true,
        KeyAction::FilterChar(c) => {
            let mut next = state.filter.clone();
            next.push(c);
            state.set_filter(next);
        }
        KeyAction::FilterBackspace => {
            let mut next = state.filter.clone();
            next.pop();
            state.set_filter(next);
        }
        KeyAction::FilterClear => {
            state.set_filter(String::new());
            state.filtering = false;
        }
        KeyAction::FilterConfirm => state.filtering = false,
        KeyAction::PickerNext => state.picker_next(),
        KeyAction::PickerPrev => state.picker_prev(),
        KeyAction::PickerApply => state.apply_picker(),
        KeyAction::PickerCancel => state.cancel_picker(),
        KeyAction::Rescan => rescan(state, cfg, ctx),
        KeyAction::ToggleGraph => state.toggle_graph(),
        KeyAction::CycleMetric => state.cycle_graph_metric(),
        KeyAction::Ignore => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_key_normal_mode() {
        assert_eq!(
            classify_key(KeyCode::Char('q'), false, false, false),
            KeyAction::Quit
        );
        assert_eq!(
            classify_key(KeyCode::Tab, false, false, false),
            KeyAction::NextTab
        );
        assert_eq!(
            classify_key(KeyCode::Char('w'), false, false, false),
            KeyAction::CycleWindow
        );
        assert_eq!(
            classify_key(KeyCode::Char('s'), false, false, false),
            KeyAction::CycleSort
        );
        assert_eq!(
            classify_key(KeyCode::Char('p'), false, false, false),
            KeyAction::OpenProjectPicker
        );
        assert_eq!(
            classify_key(KeyCode::Char('/'), false, false, false),
            KeyAction::StartFilter
        );
        assert_eq!(
            classify_key(KeyCode::Char('R'), false, false, false),
            KeyAction::Rescan
        );
        assert_eq!(
            classify_key(KeyCode::Char('z'), false, false, false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn classify_key_filtering_and_picking_modes_take_priority() {
        assert_eq!(
            classify_key(KeyCode::Char('w'), true, false, false),
            KeyAction::FilterChar('w')
        );
        assert_eq!(
            classify_key(KeyCode::Esc, true, false, false),
            KeyAction::FilterClear
        );
        // picking wins even if filtering is also somehow set.
        assert_eq!(
            classify_key(KeyCode::Char('j'), true, true, false),
            KeyAction::PickerNext
        );
        assert_eq!(
            classify_key(KeyCode::Enter, false, true, false),
            KeyAction::PickerApply
        );
        assert_eq!(
            classify_key(KeyCode::Esc, false, true, false),
            KeyAction::PickerCancel
        );
    }

    #[test]
    fn classify_key_graph_mode_is_outermost_modal() {
        // In graph mode, g/Esc close; q quits; navigation is ignored.
        assert_eq!(
            classify_key(KeyCode::Char('g'), false, false, true),
            KeyAction::ToggleGraph
        );
        assert_eq!(
            classify_key(KeyCode::Esc, false, false, true),
            KeyAction::ToggleGraph
        );
        assert_eq!(
            classify_key(KeyCode::Char('q'), false, false, true),
            KeyAction::Quit
        );
        assert_eq!(
            classify_key(KeyCode::Char('j'), false, false, true),
            KeyAction::Ignore
        );
        // Graph beats filtering/picking flags if somehow both set.
        assert_eq!(
            classify_key(KeyCode::Char('g'), true, true, true),
            KeyAction::ToggleGraph
        );
    }

    #[test]
    fn classify_key_g_opens_graph_in_normal_mode() {
        assert_eq!(
            classify_key(KeyCode::Char('g'), false, false, false),
            KeyAction::ToggleGraph
        );
    }
}
