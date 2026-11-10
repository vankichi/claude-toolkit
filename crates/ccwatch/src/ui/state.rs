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

/// The entry below `current` (an `entries` index) in the *display* order,
/// wrapping at the end.
///
/// Navigation must follow what the user sees, not the raw `entries`
/// storage order — otherwise the highlight jumps to non-adjacent rows,
/// because the table is sorted by recent activity while `entries` keeps
/// discovery order.
pub(super) fn next_selected(entries: &[SessionEntry], current: usize) -> usize {
    cctk::cycle::next(&display_order(entries), current)
}

/// The entry above `current` in the display order (see [`next_selected`]).
pub(super) fn prev_selected(entries: &[SessionEntry], current: usize) -> usize {
    cctk::cycle::prev(&display_order(entries), current)
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
