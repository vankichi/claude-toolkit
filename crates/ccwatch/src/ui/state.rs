//! View mode and display-order navigation over the session entries.

use super::SessionEntry;
use crate::summary::{self, EntryView};

/// Which view is active. Pinned to `Single` when the user passes
/// `--session <path>` (the entry list is then a single non-toggle-able row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ViewMode {
    Summary,
    Single,
}

/// Step `current` (an `entries` index) by `delta` (±1) through the
/// *display* order, wrapping at the ends. `order` is the entries-index
/// sequence in the order the summary table renders its rows (see
/// [`summary::sort_order`]).
///
/// Navigation must follow what the user sees, not the raw `entries`
/// storage order — otherwise the highlight jumps to non-adjacent rows,
/// because the table is sorted by recent activity while `entries` keeps
/// discovery order.
fn step_in_order(order: &[usize], current: usize, delta: isize) -> usize {
    if order.is_empty() {
        return current;
    }
    // Locate `current` within the display order, step by `delta` there, then
    // map back to the entries-index at the new visible position. `current` is
    // always a valid selection so it appears in `order`; the fallback keeps us
    // on the first visible row if it somehow doesn't.
    let pos = order.iter().position(|&i| i == current).unwrap_or(0);
    #[expect(clippy::cast_possible_wrap)]
    let n = order.len() as isize;
    #[expect(clippy::cast_possible_wrap)]
    let new_pos = (pos as isize + delta).rem_euclid(n) as usize;
    order[new_pos]
}

pub(super) fn next_selected(entries: &[SessionEntry], current: usize) -> usize {
    step_in_order(&display_order(entries), current, 1)
}

pub(super) fn prev_selected(entries: &[SessionEntry], current: usize) -> usize {
    step_in_order(&display_order(entries), current, -1)
}

/// The `entries`-index order in which the summary table renders its rows.
/// Shared by rendering and navigation so j/k move between visually
/// adjacent rows.
pub(super) fn display_order(entries: &[SessionEntry]) -> Vec<usize> {
    summary::sort_order(&build_views(entries))
}

/// Borrow each [`SessionEntry`] as the renderable [`EntryView`] shape.
/// Extracted so both `draw_frame` and navigation compute the same view /
/// sort order from one place.
pub(super) fn build_views(entries: &[SessionEntry]) -> Vec<EntryView<'_>> {
    entries
        .iter()
        .map(|e| EntryView {
            stats: &e.stats,
            agent_status: e.status,
            name: e.name.as_deref(),
            short_id: &e.short_id,
            path: &e.path,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_in_order_moves_along_display_order() {
        // Display order (entries-indices, as rendered top→bottom) is [2, 1, 0].
        // Stepping must follow THIS order, not the raw entries index — that's
        // the bug where j/k jumped to non-adjacent rows.
        let order = [2usize, 1, 0];
        // From entries-index 2 (top visible row), next → the row below it = 1.
        assert_eq!(step_in_order(&order, 2, 1), 1);
        // From entries-index 0 (last visible row), next wraps to top = 2.
        assert_eq!(step_in_order(&order, 0, 1), 2);
        // From entries-index 2 (top), prev wraps to the bottom = 0.
        assert_eq!(step_in_order(&order, 2, -1), 0);
        // Middle row steps normally in both directions.
        assert_eq!(step_in_order(&order, 1, 1), 0);
        assert_eq!(step_in_order(&order, 1, -1), 2);
    }

    #[test]
    fn step_in_order_empty_is_noop() {
        assert_eq!(step_in_order(&[], 0, 1), 0);
        assert_eq!(step_in_order(&[], 3, -1), 3);
    }
}
