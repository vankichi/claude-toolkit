//! Rendering of the focus board from a [`View`] (plain data, so layouts are
//! snapshot-tested with `TestBackend`). [`draw`] is the full screen (the
//! `ccfocus` TUI and cctop's drill-down); [`draw_compact`] fills cctop's
//! overview panel.
//!
//! ```text
//! focus · main:0               parallel 4 / 4 · next round 14:30 (in 6 min)
//! ┌ next up · waiting 3m ────────────────┐ ┌ today ────────────┐
//! │[1] repo-a / tmux status line · …     │ │tasks done      1  │
//! └──────────────────────────────────────┘ │…                  │
//!  #  status   repo   project   worktree   start  updated
//! ```

use crate::board::{Board, Row, Shown, age};
use crate::repos::Candidate;
use crate::text::{fit, pad, pad_left, width};
use chrono::{DateTime, FixedOffset};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::fmt::Write as _;

/// From this width the day's numbers get their own box on the right.
const WIDE: u16 = 110;
/// Outer width of that box.
const TODAY_W: u16 = 27;
/// Candidate rows shown before "… N more".
const MAX_CANDIDATES: usize = 4;

const NUM_W: usize = 2;
const STATUS_W: usize = 8;
const REPO_W: usize = 12;
const WORKTREE_W: usize = 14;
const START_W: usize = 5;
const UPDATED_W: usize = 7;
const MIN_PROJECT_W: usize = 6;

pub const HELP: &str = "1-9 jump · j/k select · d diff (hunk) · p name window · r reload · q quit";

/// Today's finished tasks, from gh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCount {
    /// The first query hasn't returned yet.
    Pending,
    /// gh is missing or failed.
    Unknown,
    Known(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Today {
    pub tasks: TaskCount,
    pub switches: u32,
    pub yesterday_switches: u32,
    pub parallel_avg: f64,
    pub parallel_max: usize,
    /// Switches per day, oldest first, ending today.
    pub week: [u32; 7],
}

/// Everything the board shows, computed once per refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub now: DateTime<FixedOffset>,
    /// `focus`, or `focus · <session>:<window>` inside tmux.
    pub title: String,
    /// Sessions with an event in the last 30 minutes.
    pub parallel: usize,
    pub limit: usize,
    /// Next check-in (`HH:MM`) and minutes left.
    pub round: Option<(String, i64)>,
    pub board: Board,
    pub cursor: Option<usize>,
    pub today: Today,
    /// `None` until the first worktree scan returns.
    pub candidates: Option<Vec<Candidate>>,
    /// PR heads were fetched, so candidates exclude branches with a PR.
    pub pr_known: bool,
    /// Transient key feedback.
    pub message: Option<String>,
    /// A config problem (defaults in use).
    pub warning: Option<String>,
}

fn status_style(s: Shown) -> Style {
    match s {
        Shown::Waiting => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        Shown::Failed => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        Shown::Done => Style::default().fg(Color::Green),
        Shown::Working => Style::default().fg(Color::Cyan),
        Shown::Idle => Style::default().fg(Color::Gray),
        Shown::Stopped => Style::default().fg(Color::DarkGray),
    }
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// `parallel N / L` styled by how it compares with the limit: red over it,
/// yellow at it.
fn parallel_span(v: &View) -> Span<'static> {
    let text = format!("parallel {} / {}", v.parallel, v.limit);
    let style = match v.parallel.cmp(&v.limit) {
        std::cmp::Ordering::Greater => Style::default()
            .fg(Color::White)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD),
        std::cmp::Ordering::Equal => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        std::cmp::Ordering::Less => Style::default(),
    };
    Span::styled(text, style)
}

fn round_text(v: &View) -> Option<String> {
    v.round
        .as_ref()
        .map(|(at, left)| format!("next round {at} (in {left} min)"))
}

/// Full-screen board.
pub fn draw(f: &mut Frame<'_>, area: Rect, v: &View) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, header, v);
    draw_footer(f, footer, v);

    let cand_h = candidates_height(v);
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
        Paragraph::new(Line::from(Span::styled(
            v.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        area,
    );
    f.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        area,
    );
}

fn draw_footer(f: &mut Frame<'_>, area: Rect, v: &View) {
    let w = usize::from(area.width);
    let line = if let Some(m) = &v.message {
        Span::styled(fit(m, w), Style::default().fg(Color::Yellow))
    } else if let Some(m) = &v.warning {
        Span::styled(fit(m, w), Style::default().fg(Color::Red))
    } else {
        Span::styled(fit(HELP, w), dim())
    };
    f.render_widget(Paragraph::new(Line::from(line)), area);
}

/// `[n] repo / project · branch · tmux`, the one-line summary of a row.
fn summary(r: &Row) -> String {
    let mut parts = vec![if r.project.is_empty() {
        r.repo.clone()
    } else {
        format!("{} / {}", r.repo, r.project)
    }];
    parts.extend(r.branch.clone());
    parts.extend(r.tmux.clone());
    let num = r
        .number
        .map_or_else(|| "[-]".to_string(), |n| format!("[{n}]"));
    format!("{num} {}", parts.join(" · "))
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(dim())
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let body = match next {
        Some(r) => Line::from(Span::styled(
            fit(&summary(r), usize::from(inner.width)),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        None => Line::from(Span::styled("none", dim())),
    };
    f.render_widget(Paragraph::new(body), inner);
}

struct Columns {
    project: usize,
    worktree: bool,
}

fn columns(total: usize) -> Columns {
    let fixed = NUM_W + STATUS_W + REPO_W + START_W + UPDATED_W + 5;
    if total > fixed + WORKTREE_W + 1 + MIN_PROJECT_W {
        Columns {
            project: total - fixed - WORKTREE_W - 2,
            worktree: true,
        }
    } else {
        Columns {
            project: total.saturating_sub(fixed + 1).max(MIN_PROJECT_W),
            worktree: false,
        }
    }
}

fn start_label(v: &View, r: &Row) -> String {
    let started = r.started_at.with_timezone(&v.now.timezone());
    if started.date_naive() == v.now.date_naive() {
        started.format("%H:%M").to_string()
    } else {
        started.format("%m/%d").to_string()
    }
}

fn list_line(v: &View, r: &Row, c: &Columns, selected: bool) -> Line<'static> {
    let num = r.number.map_or_else(|| "-".to_string(), |n| n.to_string());
    let mut spans = vec![
        Span::raw(format!("{num:>NUM_W$} ")),
        Span::styled(pad(r.shown.label(), STATUS_W), status_style(r.shown)),
        Span::raw(format!(" {} ", pad(&r.repo, REPO_W))),
        Span::styled(
            pad(
                if r.project.is_empty() {
                    "-"
                } else {
                    &r.project
                },
                c.project,
            ),
            if r.project.is_empty() {
                dim()
            } else {
                Style::default()
            },
        ),
    ];
    if c.worktree {
        spans.push(Span::raw(format!(" {}", pad(&r.worktree, WORKTREE_W))));
    }
    spans.push(Span::raw(format!(
        " {} {:>UPDATED_W$}",
        pad(&start_label(v, r), START_W),
        age(v.now - r.updated_at)
    )));
    let mut line = Line::from(spans);
    if selected {
        line = line.style(Style::default().add_modifier(Modifier::REVERSED));
    }
    line
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

fn candidates_height(v: &View) -> u16 {
    let n = v.candidates.as_ref().map_or(0, Vec::len);
    let rows = n.min(MAX_CANDIDATES) + usize::from(n > MAX_CANDIDATES);
    u16::try_from(1 + rows).unwrap_or(u16::MAX)
}

fn draw_candidates(f: &mut Frame<'_>, area: Rect, v: &View) {
    let w = usize::from(area.width);
    let bold = Style::default().add_modifier(Modifier::BOLD);
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
    let mut lines = vec![Line::from(Span::styled(fit(&title, w), bold))];
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

fn task_text(t: TaskCount) -> String {
    match t {
        TaskCount::Pending => "…".to_string(),
        TaskCount::Unknown => "n/a".to_string(),
        TaskCount::Known(n) => n.to_string(),
    }
}

/// Eight-level bars, scaled to the week's busiest day.
#[must_use]
pub fn sparkline(values: &[u32]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().copied().max().unwrap_or(0);
    values
        .iter()
        .map(|&v| {
            if max == 0 {
                BARS[0]
            } else {
                BARS[usize::try_from(u64::from(v) * 7 / u64::from(max)).unwrap_or(7)]
            }
        })
        .collect()
}

fn parallel_text(t: &Today) -> String {
    format!("{:.1} / {}", t.parallel_avg, t.parallel_max)
}

fn draw_today_box(f: &mut Frame<'_>, area: Rect, v: &View) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(dim())
        .title(" today ");
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
    use super::*;
    use cctk::focus::HostKind;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
    use std::path::PathBuf;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        number: Option<u8>,
        shown: Shown,
        repo: &str,
        project: &str,
        worktree: &str,
        branch: Option<&str>,
        tmux: Option<&str>,
        started: &str,
        updated: &str,
    ) -> Row {
        Row {
            number,
            shown,
            session_id: format!("{repo}-{worktree}"),
            host: HostKind::Host,
            local: true,
            repo: repo.into(),
            project: project.into(),
            worktree: worktree.into(),
            branch: branch.map(str::to_string),
            tmux: tmux.map(str::to_string),
            tmux_pane: tmux.map(|_| "%1".to_string()),
            repo_path: Some(PathBuf::from(format!("/work/{repo}"))),
            worktree_path: Some(PathBuf::from(format!("/work/{repo}/{worktree}"))),
            started_at: at(started),
            updated_at: at(updated),
        }
    }

    /// Synthetic board: every name and value here is made up.
    fn sample() -> View {
        let rows = vec![
            row(
                Some(1),
                Shown::Waiting,
                "repo-a",
                "tmux status line",
                "feat-tmux",
                Some("feat/tmux"),
                Some("dev:2.1"),
                "2026-01-02T10:00:00+09:00",
                "2026-01-05T14:21:00+09:00",
            ),
            row(
                Some(2),
                Shown::Done,
                "repo-b",
                "e2e 安定化",
                "e2e-388",
                Some("fix/e2e-388"),
                Some("dev:3.1"),
                "2025-12-31T09:00:00+09:00",
                "2026-01-05T14:12:00+09:00",
            ),
            row(
                Some(3),
                Shown::Working,
                "repo-c",
                "token auth",
                "guard-token",
                Some("feat/guard-token"),
                Some("dev:4.1"),
                "2026-01-03T11:00:00+09:00",
                "2026-01-05T14:23:20+09:00",
            ),
            row(
                Some(4),
                Shown::Working,
                "repo-a",
                "",
                "main",
                Some("main"),
                None,
                "2026-01-05T09:12:00+09:00",
                "2026-01-05T14:22:00+09:00",
            ),
            row(
                None,
                Shown::Stopped,
                "repo-d",
                "renovate batching",
                "renovate-2",
                Some("chore/renovate"),
                None,
                "2025-12-29T15:00:00+09:00",
                "2026-01-05T08:00:00+09:00",
            ),
        ];
        View {
            now: at("2026-01-05T14:24:00+09:00"),
            title: "focus · main:0".into(),
            parallel: 5,
            limit: 4,
            round: Some(("14:30".into(), 6)),
            board: Board {
                rows,
                folded: 2,
                next: Some(0),
            },
            cursor: Some(0),
            today: Today {
                tasks: TaskCount::Known(1),
                switches: 12,
                yesterday_switches: 56,
                parallel_avg: 2.4,
                parallel_max: 5,
                week: [31, 48, 56, 22, 40, 56, 12],
            },
            candidates: Some(vec![
                Candidate {
                    repo: "repo-a".into(),
                    worktree: "guard-egress".into(),
                    ahead: 1,
                    behind: 5,
                    dirty: Some(0),
                    sessions: vec![],
                },
                Candidate {
                    repo: "repo-c".into(),
                    worktree: "guard-token".into(),
                    ahead: 3,
                    behind: 0,
                    dirty: Some(2),
                    sessions: vec![3],
                },
            ]),
            pr_known: true,
            message: None,
            warning: None,
        }
    }

    fn render(v: &View, w: u16, h: u16, compact: bool) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| {
                if compact {
                    draw_compact(f, f.area(), v);
                } else {
                    draw(f, f.area(), v);
                }
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// The buffer as text, skipping the cells hidden behind wide characters.
    fn text(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            let mut skip = 0;
            let mut line = String::new();
            for x in 0..buf.area.width {
                let sym = buf[(x, y)].symbol();
                if skip == 0 {
                    line.push_str(sym);
                }
                skip = skip.max(width(sym)).saturating_sub(1);
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// Compare with `tests/snapshots/<name>.txt`; `UPDATE_SNAPSHOTS=1`
    /// rewrites the file instead.
    fn snapshot(name: &str, got: &str) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/snapshots")
            .join(format!("{name}.txt"));
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, got).unwrap();
            return;
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing {}; run with UPDATE_SNAPSHOTS=1", path.display()))
            // A Windows checkout with core.autocrlf turns the file's LF into CRLF.
            .replace("\r\n", "\n");
        assert_eq!(got, want, "snapshot {name} differs");
    }

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
    fn snapshot_compact_panel() {
        snapshot("panel_100", &text(&render(&sample(), 100, 6, true)));
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

    fn find(buf: &Buffer, needle: &str) -> (u16, u16) {
        let t = text(buf);
        let (y, line) = t
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not drawn"));
        // Columns before the match (wide characters count double).
        let col = width(&line[..line.find(needle).unwrap()]);
        (u16::try_from(col).unwrap(), u16::try_from(y).unwrap())
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

    #[test]
    fn sparkline_scales_to_the_max() {
        assert_eq!(sparkline(&[0, 7, 14]), "▁▄█");
        assert_eq!(sparkline(&[0, 0]), "▁▁");
        assert_eq!(sparkline(&[]), "");
    }
}
