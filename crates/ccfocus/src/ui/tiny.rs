//! The card view for a narrow side pane (`ccfocus tui --tiny`).

use super::{
    TINY_HELP, View, candidates_height, dim, draw_footer, parallel_style, sessions_suffix,
    start_label, status_style, task_text, week_span,
};
use crate::board::{Row, age};
use crate::text::{fit, pad, width};
use cctk::style::{bold, panel};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{BorderType, Paragraph},
};
use std::fmt::Write as _;

/// Rows of one card in the tiny view: border, repo, project, border.
const CARD_H: u16 = 4;
/// Worktrees to finish listed in the tiny view before "+N more".
const MAX_TINY_CANDIDATES: usize = 2;
/// Lines of the day's numbers in the tiny view.
const TINY_TODAY_H: u16 = 2;

/// Narrow board for a side pane: `N / L · HH:MM (Nm)` and the day's numbers,
/// one card per session (the cursor's card thick, in its status colour and
/// one line taller for branch, tmux and start; the next-up card marked `▸`),
/// the worktrees to finish, and a short help line. In a short pane the
/// worktrees to finish, then the day's numbers, give way to the cursor's card.
pub fn draw_tiny(f: &mut Frame<'_>, area: Rect, v: &View) {
    // The cursor's card and the hidden-count line, or the empty-board line.
    let need = if v.board.rows.is_empty() {
        1
    } else {
        CARD_H + 1 + u16::from(v.cursor.is_some())
    };
    let spare = area.height.saturating_sub(2 + need);
    let today_h = TINY_TODAY_H.min(spare);
    let finish_h =
        candidates_height(v, MAX_TINY_CANDIDATES).min(spare.saturating_sub(TINY_TODAY_H));
    let [header, today, body, to_finish, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(today_h),
        Constraint::Min(0),
        Constraint::Length(finish_h),
        Constraint::Length(1),
    ])
    .areas(area);
    let w = usize::from(area.width);
    let limit = format!("{} / {}", v.parallel, v.limit);
    let round = v
        .round
        .as_ref()
        .map_or_else(String::new, |(at, left)| format!(" · {at} ({left}m)"));
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(fit(&limit, w), parallel_style(v)),
            Span::raw(fit(&round, w.saturating_sub(width(&limit)))),
        ])),
        header,
    );
    draw_tiny_today(f, today, v);
    draw_tiny_candidates(f, to_finish, v);
    draw_footer(f, footer, v, TINY_HELP);

    let rows = &v.board.rows;
    if rows.is_empty() && v.board.folded == 0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(fit("no sessions yet", w), dim()))),
            body,
        );
        return;
    }
    // The cursor's card is one line taller.
    let room = body.height.saturating_sub(u16::from(v.cursor.is_some()));
    let cards = usize::from(room.saturating_sub(u16::from(v.board.folded > 0)) / CARD_H);
    // Keep a line for the hidden count when not every card fits, unless that
    // would leave no card at all.
    let shown = if rows.len() <= cards {
        rows.len()
    } else {
        usize::from(room.saturating_sub(1) / CARD_H).max(cards.min(1))
    };
    // Scroll so the cursor's card (the one d / p / Enter act on) is drawn.
    let first = v
        .cursor
        .map_or(0, |c| (c + 1).saturating_sub(shown))
        .min(rows.len() - shown);
    let mut y = body.y;
    for (i, r) in rows.iter().enumerate().skip(first).take(shown) {
        let selected = v.cursor == Some(i);
        let card_h = CARD_H + u16::from(selected);
        let card = Rect::new(body.x, y, body.width, card_h);
        draw_card(f, card, v, r, selected, v.board.next == Some(i));
        y += card_h;
    }
    // Hidden cards above (scrolled past) and below the drawn ones, then the
    // folded ones, on one line.
    let mut tail = Vec::new();
    match (first, rows.len() - first - shown) {
        (0, 0) => {}
        (up, 0) => tail.push(format!("↑{up} more")),
        (0, down) => tail.push(format!("↓{down} more")),
        (up, down) => tail.push(format!("↑{up} ↓{down} more")),
    }
    if v.board.folded > 0 {
        tail.push(format!("+{} old", v.board.folded));
    }
    let tail_h = (body.y + body.height).saturating_sub(y);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(fit(&tail.join(" · "), w), dim()))),
        Rect::new(body.x, y, body.width, tail_h.min(1)),
    );
}

/// `done N · sw N (y N)` over `par avg/max` and the week's sparkline.
fn draw_tiny_today(f: &mut Frame<'_>, area: Rect, v: &View) {
    let w = usize::from(area.width);
    let t = &v.today;
    let counts = format!(
        "done {} · sw {} (y {})",
        task_text(t.tasks),
        t.switches,
        t.yesterday_switches
    );
    let par = format!("par {:.1}/{} ", t.parallel_avg, t.parallel_max);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(fit(&counts, w)),
            Line::from(vec![
                Span::raw(fit(&par, w)),
                week_span(&t.week, w.saturating_sub(width(&par))),
            ]),
        ]),
        area,
    );
}

/// `to finish N` and ` repo·worktree ↑1 ↓5 ±2 [3]` per worktree, the first
/// few only.
fn draw_tiny_candidates(f: &mut Frame<'_>, area: Rect, v: &View) {
    let w = usize::from(area.width);
    let Some(cands) = &v.candidates else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(fit("to finish …", w), dim()))),
            area,
        );
        return;
    };
    let mut title = format!("to finish {}", cands.len());
    if !v.pr_known {
        title.push_str(" · PR?");
    }
    let mut lines = vec![Line::from(Span::styled(fit(&title, w), bold()))];
    for c in cands.iter().take(MAX_TINY_CANDIDATES) {
        let mut tail = format!(" ↑{}", c.ahead);
        if c.behind > 0 {
            let _ = write!(tail, " ↓{}", c.behind);
        }
        match c.dirty {
            Some(0) => {}
            Some(n) => {
                let _ = write!(tail, " ±{n}");
            }
            None => tail.push_str(" ?"),
        }
        tail.push_str(&sessions_suffix(c));
        // The place gives way first, so the numbers stay readable.
        let place = fit(
            &format!("{}·{}", c.repo, c.worktree),
            w.saturating_sub(1 + width(&tail)),
        );
        lines.push(Line::from(fit(&format!(" {place}{tail}"), w)));
    }
    if cands.len() > MAX_TINY_CANDIDATES {
        lines.push(Line::from(Span::styled(
            fit(&format!(" +{} more", cands.len() - MAX_TINY_CANDIDATES), w),
            dim(),
        )));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// `┌▸1 waiting ── 3m┐` over `repo · worktree` and the project; the cursor's
/// card adds `branch · tmux` and the start.
fn draw_card(f: &mut Frame<'_>, area: Rect, v: &View, r: &Row, selected: bool, next: bool) {
    let style = status_style(r.shown);
    let num = r.number.map_or_else(|| "-".to_string(), |n| n.to_string());
    let mark = if next { "▸" } else { "" };
    let block = panel(Line::from(vec![
        Span::raw(format!("{mark}{num} ")),
        Span::styled(r.shown.label(), style),
    ]))
    .border_type(if selected {
        BorderType::Thick
    } else {
        BorderType::Plain
    })
    .border_style(if selected {
        style.remove_modifier(Modifier::BOLD)
    } else {
        dim()
    })
    .title(Line::from(age(v.now - r.updated_at)).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = usize::from(inner.width);
    let place = if r.worktree.is_empty() || r.worktree == r.repo {
        r.repo.clone()
    } else {
        format!("{} · {}", r.repo, r.worktree)
    };
    let project = if r.project.is_empty() {
        Span::styled("-", dim())
    } else {
        Span::raw(fit(&r.project, w))
    };
    let mut lines = vec![
        Line::from(Span::styled(fit(&place, w), bold())),
        Line::from(project),
    ];
    if selected {
        let start = start_label(v, r);
        let at: Vec<&str> = r.branch.iter().chain(&r.tmux).map(String::as_str).collect();
        let room = w.saturating_sub(width(&start) + 1);
        let at = if at.is_empty() {
            Span::styled(pad("-", room), dim())
        } else {
            Span::raw(pad(&at.join(" · "), room))
        };
        lines.push(Line::from(vec![
            at,
            Span::styled(format!(" {start}"), dim()),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;
    use crate::ui::test_support::{at, render_with, sample, snapshot, text};
    use ratatui::style::Color;

    #[test]
    fn snapshot_tiny() {
        snapshot("tiny_24", &text(&render_with(&sample(), 24, 24, draw_tiny)));
    }

    // Tiny at 24 columns: header and today take 3 lines, the two candidates
    // and their title 3, the help 1; 17 lines leave a body of 10, room for
    // the cursor's card (5), one more (4) and the tail line.

    #[test]
    fn tiny_scrolls_to_the_cursor_card() {
        let mut v = sample();
        v.cursor = Some(4);
        let t = text(&render_with(&v, 24, 17, draw_tiny));
        assert!(t.contains("renovate batching"), "{t}");
        assert!(t.contains("↑3 more · +2 old"), "{t}");
        assert!(!t.contains("tmux status line"), "{t}");
    }

    #[test]
    fn tiny_counts_hidden_cards_on_both_sides() {
        let mut v = sample();
        v.cursor = Some(2);
        let t = text(&render_with(&v, 24, 17, draw_tiny));
        assert!(t.contains("↑1 ↓2 more"), "{t}");

        v.cursor = Some(0);
        let t = text(&render_with(&v, 24, 17, draw_tiny));
        assert!(t.contains("↓3 more"), "{t}");
    }

    #[test]
    fn tiny_draws_one_card_when_only_one_fits() {
        let mut v = sample();
        v.board.folded = 0;
        v.cursor = Some(1);
        let t = text(&render_with(&v, 24, 13, draw_tiny));
        assert!(t.contains("e2e 安定化"), "{t}");
        assert!(t.contains("↑1 ↓3 more"), "{t}");
    }

    #[test]
    fn tiny_short_panes_keep_the_cursor_card() {
        let mut v = sample();
        v.cursor = Some(2);
        // Header, help, the cursor's card (5) and the hidden-count line: 8.
        let t = text(&render_with(&v, 24, 8, draw_tiny));
        assert!(t.contains("┃token auth"), "{t}");
        assert!(t.contains("↑2 ↓2 more · +2 old"), "{t}");
        assert!(!t.contains("done 1"), "{t}");
        assert!(!t.contains("to finish"), "{t}");

        // The day's numbers come back before the worktrees to finish.
        let t = text(&render_with(&v, 24, 10, draw_tiny));
        assert!(t.contains("┃token auth"), "{t}");
        assert!(t.contains("par 2.4/5"), "{t}");
        assert!(!t.contains("to finish"), "{t}");

        let t = text(&render_with(&v, 24, 11, draw_tiny));
        assert!(t.contains("┃token auth"), "{t}");
        assert!(t.contains("to finish 2"), "{t}");
        assert!(!t.contains("repo-a·guard"), "{t}");
    }

    #[test]
    fn tiny_cursor_card_is_thick_in_its_status_colour() {
        let buf = render_with(&sample(), 24, 24, draw_tiny);
        let t = text(&buf);
        assert!(t.lines().nth(3).unwrap().starts_with('┏'), "{t}");
        assert!(t.lines().nth(8).unwrap().starts_with('┌'), "{t}");
        assert_eq!(buf[(0, 3)].fg, Color::Yellow);

        let mut v = sample();
        v.cursor = Some(1);
        let buf = render_with(&v, 24, 24, draw_tiny);
        assert_eq!(buf[(0, 7)].fg, Color::Green);
    }

    #[test]
    fn tiny_marks_next_up_not_the_cursor() {
        let mut v = sample();
        v.cursor = Some(1);
        let t = text(&render_with(&v, 24, 24, draw_tiny));
        assert!(t.lines().nth(3).unwrap().starts_with("┌▸1 "), "{t}");
        assert!(t.lines().nth(7).unwrap().starts_with("┏2 "), "{t}");
    }

    #[test]
    fn tiny_cursor_card_alone_shows_branch_tmux_and_start() {
        let mut v = sample();
        v.board.rows[0].branch = Some("b".into());
        v.board.rows[0].tmux = Some("t:1".into());
        v.board.rows[1].branch = Some("other-br".into());
        let t = text(&render_with(&v, 24, 24, draw_tiny));
        assert!(t.contains("┃b · t:1          01/02┃"), "{t}");
        assert!(!t.contains("other-br"), "{t}");

        // Started today: the time instead of the date.
        v.board.rows[0].started_at = at("2026-01-05T09:05:00+09:00");
        let t = text(&render_with(&v, 24, 24, draw_tiny));
        assert!(t.contains("09:05┃"), "{t}");

        v.board.rows[0].branch = None;
        v.board.rows[0].tmux = None;
        let t = text(&render_with(&v, 24, 24, draw_tiny));
        assert!(t.contains("┃-                09:05┃"), "{t}");
    }

    #[test]
    fn tiny_shows_the_round_today_and_worktrees_to_finish() {
        let t = text(&render_with(&sample(), 24, 24, draw_tiny));
        assert!(t.starts_with("5 / 4 · 14:30 (6m)\n"), "{t}");
        assert!(t.contains("done 1 · sw 12 (y 56)"), "{t}");
        assert!(t.contains("par 2.4/5 ▄▇█▃▆█▂"), "{t}");
        assert!(t.contains("to finish 2\n"), "{t}");
        // Clean worktrees and ones not behind leave those numbers out.
        assert!(t.contains("\n repo-a·guard-egr… ↑1 ↓5\n"), "{t}");
        assert!(t.contains("\n repo-c·guard… ↑3 ±2 [3]\n"), "{t}");
    }

    #[test]
    fn tiny_worktrees_to_finish_end_with_a_count() {
        let mut v = sample();
        let cands = v.candidates.as_mut().unwrap();
        cands.push(cands[0].clone());
        cands[2].dirty = None;
        v.pr_known = false;
        let t = text(&render_with(&v, 24, 24, draw_tiny));
        assert!(t.contains("to finish 3 · PR?"), "{t}");
        assert!(t.contains(" +1 more"), "{t}");

        v.candidates = None;
        let t = text(&render_with(&v, 24, 24, draw_tiny));
        assert!(t.contains("to finish …"), "{t}");
    }

    #[test]
    fn tiny_empty_board() {
        let mut v = sample();
        v.board = Board::default();
        v.cursor = None;
        let t = text(&render_with(&v, 24, 10, draw_tiny));
        assert!(t.contains("no sessions yet"), "{t}");
    }
}
