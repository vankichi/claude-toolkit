//! Fixtures and helpers shared by the layouts' tests.

use super::{TaskCount, Today, View, draw, draw_compact};
use crate::board::{Board, Row, Shown};
use crate::repos::Candidate;
use crate::text::width;
use cctk::focus::HostKind;
use chrono::{DateTime, FixedOffset};
use ratatui::{Frame, Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};
use std::path::PathBuf;

pub(super) fn at(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}

#[expect(clippy::too_many_arguments)]
pub(super) fn row(
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
        host_tmux_pane: None,
        repo_path: Some(PathBuf::from(format!("/work/{repo}"))),
        worktree_path: Some(PathBuf::from(format!("/work/{repo}/{worktree}"))),
        started_at: at(started),
        updated_at: at(updated),
    }
}

/// Synthetic board: every name and value here is made up.
pub(super) fn sample() -> View {
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

pub(super) fn render(v: &View, w: u16, h: u16, compact: bool) -> Buffer {
    render_with(v, w, h, if compact { draw_compact } else { draw })
}

pub(super) fn render_with(
    v: &View,
    w: u16,
    h: u16,
    draw_fn: fn(&mut Frame<'_>, Rect, &View),
) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| draw_fn(f, f.area(), v)).unwrap();
    terminal.backend().buffer().clone()
}

/// The buffer as text, skipping the cells hidden behind wide characters.
pub(super) fn text(buf: &Buffer) -> String {
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
pub(super) fn snapshot(name: &str, got: &str) {
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

pub(super) fn find(buf: &Buffer, needle: &str) -> (u16, u16) {
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
