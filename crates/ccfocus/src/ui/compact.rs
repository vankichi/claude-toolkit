//! cctop's overview panel.

use super::{
    View, columns, dim, list_line, parallel_span, round_text, status_style, summary, task_text,
};
use crate::board::age;
use crate::text::{fit, width};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use std::fmt::Write as _;

/// cctop's overview panel: limit and today's numbers, the next session, and
/// as many rows as fit.
pub fn draw_compact(f: &mut Frame<'_>, area: Rect, v: &View) {
    let w = usize::from(area.width);
    let mut first = vec![parallel_span(v)];
    let mut rest = String::new();
    if let Some(r) = round_text(v) {
        let _ = write!(rest, " · {r}");
    }
    let _ = write!(
        rest,
        " · today tasks done {} · switches {}",
        task_text(v.today.tasks),
        v.today.switches
    );
    first.push(Span::raw(fit(&rest, w.saturating_sub(first[0].width()))));
    let mut lines = vec![Line::from(first)];

    match v.board.next.and_then(|i| v.board.rows.get(i)) {
        Some(r) => lines.push(Line::from(vec![
            Span::raw("next: "),
            Span::styled(r.shown.label(), status_style(r.shown)),
            Span::raw(fit(
                &format!(" {} · {}", age(v.now - r.updated_at), summary(r)),
                w.saturating_sub(4 + width(r.shown.label())),
            )),
        ])),
        None => lines.push(Line::from(Span::styled("next: none", dim()))),
    }

    let room = usize::from(area.height).saturating_sub(lines.len());
    let c = columns(w);
    // No cursor here: the overview's keys select panels, not rows.
    for r in v.board.rows.iter().take(room) {
        lines.push(list_line(v, r, &c, false));
    }
    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use crate::ui::test_support::{render, sample, snapshot, text};

    #[test]
    fn snapshot_compact_panel() {
        snapshot("panel_100", &text(&render(&sample(), 100, 6, true)));
    }
}
