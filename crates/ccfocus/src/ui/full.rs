//! The full-screen board: the `ccfocus` TUI and cctop's drill-down.

use super::{
    HELP, NUM_W, REPO_W, START_W, STATUS_W, Today, UPDATED_W, View, WORKTREE_W, candidates_height,
    columns, dim, draw_footer, list_line, parallel_span, round_text, sparkline, status_style,
    summary, task_text,
};
use crate::board::age;
use crate::text::{fit, pad, pad_left, width};
use cctk::style::{bold, panel};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::fmt::Write as _;

/// From this width the day's numbers get their own box on the right.
const WIDE: u16 = 110;
/// Outer width of that box.
const TODAY_W: u16 = 27;
/// Candidate rows shown before "… N more".
const MAX_CANDIDATES: usize = 4;

/// Full-screen board.
pub fn draw(f: &mut Frame<'_>, area: Rect, v: &View) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, header, v);
    draw_footer(f, footer, v, HELP);

    let cand_h = candidates_height(v, MAX_CANDIDATES);
    if area.width >= WIDE {
        let [left, right] = Layout::horizontal([Constraint::Min(0), Constraint::Length(TODAY_W)])
            .spacing(1)
            .areas(body);
        let [next, list, cands] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(cand_h),
        ])
        .areas(left);
        draw_next(f, next, v);
        draw_list(f, list, v);
        draw_candidates(f, cands, v);
        let [today, _] = Layout::vertical([Constraint::Length(8), Constraint::Min(0)]).areas(right);
        draw_today_box(f, today, v);
    } else {
        let [next, today, list, cands] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(cand_h),
        ])
        .areas(body);
        draw_next(f, next, v);
        f.render_widget(
            Paragraph::new(today_line(v, usize::from(today.width))),
            today,
        );
        draw_list(f, list, v);
        draw_candidates(f, cands, v);
    }
}

fn draw_header(f: &mut Frame<'_>, area: Rect, v: &View) {
    let mut right = vec![parallel_span(v)];
    if let Some(r) = round_text(v) {
        right.push(Span::raw(format!(" · {r}")));
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(v.title.clone(), bold()))),
        area,
    );
    f.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        area,
    );
}

fn draw_next(f: &mut Frame<'_>, area: Rect, v: &View) {
    let next = v.board.next.and_then(|i| v.board.rows.get(i));
    let title = match next {
        Some(r) => Line::from(vec![
            Span::raw(" next up · "),
            Span::styled(r.shown.label(), status_style(r.shown)),
            Span::raw(format!(" {} ", age(v.now - r.updated_at))),
        ]),
        None => Line::from(" next up "),
    };
    let block = panel(title).border_style(dim());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let body = match next {
        Some(r) => Line::from(Span::styled(
            fit(&summary(r), usize::from(inner.width)),
            bold(),
        )),
        None => Line::from(Span::styled("none", dim())),
    };
    f.render_widget(Paragraph::new(body), inner);
}

fn draw_list(f: &mut Frame<'_>, area: Rect, v: &View) {
    let total = usize::from(area.width);
    let c = columns(total);
    let mut head = format!(
        "{:>NUM_W$} {} {} {}",
        "#",
        pad("status", STATUS_W),
        pad("repo", REPO_W),
        pad("project", c.project)
    );
    if c.worktree {
        let _ = write!(head, " {}", pad("worktree", WORKTREE_W));
    }
    let _ = write!(head, " {} {:>UPDATED_W$}", pad("start", START_W), "updated");
    let mut lines = vec![Line::from(Span::styled(fit(&head, total), dim()))];

    let room = usize::from(area.height).saturating_sub(1);
    let rows = &v.board.rows;
    let extra = usize::from(v.board.folded > 0);
    if rows.is_empty() && v.board.folded == 0 {
        lines.push(Line::from(Span::styled(
            "no sessions yet (register the ccfocus hook: see the README)",
            dim(),
        )));
    }
    let fits = room.saturating_sub(extra);
    let shown = if rows.len() > fits {
        fits.saturating_sub(1)
    } else {
        rows.len()
    };
    // Scroll so the cursor row (the one d / p / Enter act on) is drawn.
    let first = v
        .cursor
        .map_or(0, |c| (c + 1).saturating_sub(shown))
        .min(rows.len() - shown);
    for (i, r) in rows.iter().enumerate().skip(first).take(shown) {
        lines.push(list_line(v, r, &c, v.cursor == Some(i)));
    }
    if shown < rows.len() {
        lines.push(Line::from(Span::styled(
            format!("   … {} more", rows.len() - shown),
            dim(),
        )));
    }
    if v.board.folded > 0 {
        lines.push(Line::from(Span::styled(
            format!("   {} sessions done or stopped 2+ days ago", v.board.folded),
            dim(),
        )));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_candidates(f: &mut Frame<'_>, area: Rect, v: &View) {
    let w = usize::from(area.width);
    let Some(cands) = &v.candidates else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("to finish: checking…", dim()))),
            area,
        );
        return;
    };
    let mut title = "to finish (ahead of base · no PR)".to_string();
    if !v.pr_known {
        title.push_str(" · PRs unknown");
    }
    if cands.is_empty() {
        title = "to finish: none".to_string();
    }
    let mut lines = vec![Line::from(Span::styled(fit(&title, w), bold()))];
    for c in cands.iter().take(MAX_CANDIDATES) {
        let state = match c.dirty {
            Some(0) => "clean".to_string(),
            Some(n) => format!("{n} uncommitted"),
            None => "?".to_string(),
        };
        let mut text = format!(
            "   {} {} ↑{} ↓{} {state}",
            pad(&c.repo, REPO_W),
            pad(&c.worktree, WORKTREE_W),
            c.ahead,
            c.behind
        );
        if !c.sessions.is_empty() {
            let nums: Vec<String> = c.sessions.iter().map(u8::to_string).collect();
            let _ = write!(text, " [{}]", nums.join(","));
        }
        lines.push(Line::from(fit(&text, w)));
    }
    if cands.len() > MAX_CANDIDATES {
        lines.push(Line::from(Span::styled(
            format!("   … {} more", cands.len() - MAX_CANDIDATES),
            dim(),
        )));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn parallel_text(t: &Today) -> String {
    format!("{:.1} / {}", t.parallel_avg, t.parallel_max)
}

fn draw_today_box(f: &mut Frame<'_>, area: Rect, v: &View) {
    let block = panel(" today ").border_style(dim());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let t = &v.today;
    let row = |label: &str, value: String| {
        Line::from(format!("{}{}", pad(label, 17), pad_left(&value, 7)))
    };
    let lines = vec![
        row("tasks done", task_text(t.tasks)),
        row("switches", t.switches.to_string()),
        row("yesterday", t.yesterday_switches.to_string()),
        row("parallel avg/max", parallel_text(t)),
        Line::from(Span::styled("week (switches/day)", dim())),
        Line::from(Span::styled(
            sparkline(&t.week),
            Style::default().fg(Color::Cyan),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn today_line(v: &View, w: usize) -> Line<'static> {
    let t = &v.today;
    let text = format!(
        "today  done {} · switches {} (yesterday {}) · parallel {} · week ",
        task_text(t.tasks),
        t.switches,
        t.yesterday_switches,
        parallel_text(t)
    );
    let room = w.saturating_sub(width(&text));
    Line::from(vec![
        Span::raw(fit(&text, w)),
        Span::styled(
            fit(&sparkline(&t.week), room),
            Style::default().fg(Color::Cyan),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Board, Shown};
    use crate::ui::TaskCount;
    use crate::ui::test_support::{find, render, sample, snapshot, text};

    #[test]
    fn snapshot_width_80() {
        snapshot("board_80", &text(&render(&sample(), 80, 24, false)));
    }

    #[test]
    fn snapshot_width_120() {
        snapshot("board_120", &text(&render(&sample(), 120, 24, false)));
    }

    #[test]
    fn snapshot_width_160() {
        snapshot("board_160", &text(&render(&sample(), 160, 24, false)));
    }

    #[test]
    fn snapshot_empty_board() {
        let mut v = sample();
        v.board = Board::default();
        v.cursor = None;
        v.parallel = 0;
        v.round = None;
        v.candidates = None;
        v.today.tasks = TaskCount::Unknown;
        v.warning = Some("config.toml: line 1: unknown key `colour`".into());
        snapshot("board_empty_80", &text(&render(&v, 80, 12, false)));
    }

    #[test]
    fn over_the_limit_is_red_and_at_it_is_yellow() {
        let buf = render(&sample(), 120, 24, false);
        let (x, y) = find(&buf, "parallel 5 / 4");
        assert_eq!(buf[(x, y)].bg, Color::Red);

        let mut v = sample();
        v.parallel = 4;
        let buf = render(&v, 120, 24, false);
        let (x, y) = find(&buf, "parallel 4 / 4");
        assert_eq!(buf[(x, y)].fg, Color::Yellow);
        assert_eq!(buf[(x, y)].bg, Color::Reset);
    }

    #[test]
    fn failed_rows_are_red() {
        let mut v = sample();
        v.board.rows[1].shown = Shown::Failed;
        let buf = render(&v, 120, 24, false);
        let (x, y) = find(&buf, "failed");
        assert_eq!(buf[(x, y)].fg, Color::Red);
    }

    #[test]
    fn message_replaces_the_help_line() {
        let mut v = sample();
        v.message = Some("that session is not in tmux".into());
        let t = text(&render(&v, 80, 24, false));
        assert!(t.contains("that session is not in tmux"));
        assert!(!t.contains("r reload"));
    }

    #[test]
    fn long_lists_end_with_a_count() {
        let mut v = sample();
        let extra = v.board.rows[2].clone();
        v.board.rows.extend(std::iter::repeat_n(extra, 20));
        let t = text(&render(&v, 80, 24, false));
        assert!(t.contains(" more"), "{t}");
        assert!(t.contains("2 sessions done or stopped 2+ days ago"));
    }

    #[test]
    fn unknown_tasks_fit_the_today_box() {
        let mut v = sample();
        v.today.tasks = TaskCount::Unknown;
        let t = text(&render(&v, 120, 24, false));
        assert!(t.contains("│tasks done           n/a │"), "{t}");
    }

    #[test]
    fn the_cursor_row_stays_on_screen() {
        let mut v = sample();
        let extra = v.board.rows[2].clone();
        for i in 0..20 {
            let mut r = extra.clone();
            r.project = format!("extra-{i:02}");
            v.board.rows.push(r);
        }
        v.cursor = Some(v.board.rows.len() - 1);
        let t = text(&render(&v, 80, 24, false));
        assert!(t.contains("extra-19"), "{t}");
        assert!(
            !t.contains("tmux status line  "),
            "scrolled past the first row"
        );
    }
}
