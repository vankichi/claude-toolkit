//! Rendering of the focus board from a [`View`] (plain data, so layouts are
//! snapshot-tested with `TestBackend`). [`draw`] is the full screen (the
//! `ccfocus` TUI and cctop's drill-down); [`draw_compact`] fills cctop's
//! overview panel; [`draw_tiny`] is the card view for a narrow side pane
//! (`ccfocus tui --tiny`).
//!
//! ```text
//! focus · main:0               parallel 4 / 4 · next round 14:30 (in 6 min)
//! ┌ next up · waiting 3m ────────────────┐ ┌ today ────────────┐
//! │[1] repo-a / tmux status line · …     │ │tasks done      1  │
//! └──────────────────────────────────────┘ │…                  │
//!  #  status   repo   project   worktree   start  updated
//! ```
//!
//! This file holds the data types and what more than one layout draws with;
//! each layout lives in its own file.

mod compact;
mod full;
#[cfg(test)]
mod test_support;
mod tiny;

pub use compact::draw_compact;
pub use full::draw;
pub use tiny::draw_tiny;

use crate::board::{Board, Row, Shown, age};
use crate::repos::Candidate;
use crate::text::{fit, pad};
use cctk::style::{bold_fg, dim, reversed};
use chrono::{DateTime, FixedOffset};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

const NUM_W: usize = 2;
const STATUS_W: usize = 8;
const REPO_W: usize = 12;
const WORKTREE_W: usize = 14;
const START_W: usize = 5;
const UPDATED_W: usize = 7;
const MIN_PROJECT_W: usize = 6;

pub const HELP: &str = "1-9 jump · j/k select · d diff (hunk) · p name window · r reload · q quit";
pub const TINY_HELP: &str = "1-9 j/k d p r q";

/// Today's finished tasks, from gh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskCount {
    /// The first query hasn't returned yet.
    #[default]
    Pending,
    /// gh is missing or failed.
    Unknown,
    Known(usize),
}

/// Today's numbers; the default is the board before anything was read.
#[derive(Debug, Clone, PartialEq, Default)]
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
        Shown::Waiting => bold_fg(Color::Yellow),
        Shown::Failed => bold_fg(Color::Red),
        Shown::Done => Style::default().fg(Color::Green),
        Shown::Working => Style::default().fg(Color::Cyan),
        Shown::Idle => Style::default().fg(Color::Gray),
        Shown::Stopped => dim(),
    }
}

/// `parallel N / L` styled by how it compares with the limit: red over it,
/// yellow at it.
fn parallel_span(v: &View) -> Span<'static> {
    Span::styled(
        format!("parallel {} / {}", v.parallel, v.limit),
        parallel_style(v),
    )
}

fn parallel_style(v: &View) -> Style {
    match v.parallel.cmp(&v.limit) {
        std::cmp::Ordering::Greater => Style::default()
            .fg(Color::White)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD),
        std::cmp::Ordering::Equal => bold_fg(Color::Yellow),
        std::cmp::Ordering::Less => Style::default(),
    }
}

fn round_text(v: &View) -> Option<String> {
    v.round
        .as_ref()
        .map(|(at, left)| format!("next round {at} (in {left} min)"))
}

fn draw_footer(f: &mut Frame<'_>, area: Rect, v: &View, help: &str) {
    let w = usize::from(area.width);
    let line = if let Some(m) = &v.message {
        Span::styled(fit(m, w), Style::default().fg(Color::Yellow))
    } else if let Some(m) = &v.warning {
        Span::styled(fit(m, w), Style::default().fg(Color::Red))
    } else {
        Span::styled(fit(help, w), dim())
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
        line = line.style(reversed());
    }
    line
}

/// The title, up to `max` candidates, and the "… N more" line.
fn candidates_height(v: &View, max: usize) -> u16 {
    let n = v.candidates.as_ref().map_or(0, Vec::len);
    let rows = n.min(max) + usize::from(n > max);
    u16::try_from(1 + rows).unwrap_or(u16::MAX)
}

fn task_text(t: TaskCount) -> String {
    match t {
        TaskCount::Pending => "…".to_string(),
        TaskCount::Unknown => "n/a".to_string(),
        TaskCount::Known(n) => n.to_string(),
    }
}

/// ` [1,2]`: the numbers of the sessions working in a candidate, or nothing
/// when none is.
fn sessions_suffix(c: &Candidate) -> String {
    if c.sessions.is_empty() {
        return String::new();
    }
    let nums: Vec<String> = c.sessions.iter().map(u8::to_string).collect();
    format!(" [{}]", nums.join(","))
}

/// The week's sparkline in cyan, cut to `max` columns (`usize::MAX` when the
/// caller has room for all of it).
fn week_span(week: &[u32], max: usize) -> Span<'static> {
    Span::styled(fit(&sparkline(week), max), Style::default().fg(Color::Cyan))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_scales_to_the_max() {
        assert_eq!(sparkline(&[0, 7, 14]), "▁▄█");
        assert_eq!(sparkline(&[0, 0]), "▁▁");
        assert_eq!(sparkline(&[]), "");
    }

    #[test]
    fn sessions_suffix_lists_numbers_or_nothing() {
        let mut c = Candidate {
            repo: "r".into(),
            worktree: "w".into(),
            ahead: 1,
            behind: 0,
            dirty: None,
            sessions: vec![],
        };
        assert_eq!(sessions_suffix(&c), "");
        c.sessions = vec![1, 2];
        assert_eq!(sessions_suffix(&c), " [1,2]");
    }

    #[test]
    fn week_span_is_cyan_and_cut_to_the_room() {
        let week = [0, 7, 14, 0, 0, 0, 0];
        let cyan = Style::default().fg(Color::Cyan);
        assert_eq!(week_span(&week, usize::MAX), Span::styled("▁▄█▁▁▁▁", cyan));
        assert_eq!(week_span(&week, 3), Span::styled("▁▄…", cyan));
    }
}
