//! The session list: which sessions are still alive, how they are ordered and
//! numbered, and which one to look at next. Pure; the caller supplies the
//! state files, the time, and a pid check.

use crate::text;
use cctk::focus::{HostKind, SessionState, Status};
use chrono::{DateTime, FixedOffset, TimeDelta};
use std::collections::HashSet;
use std::path::PathBuf;

/// A session whose pid can't be checked from here (the other side of the
/// host / container boundary, or no answer from `kill`) counts as stopped
/// after this long without an event.
pub const STALE_AFTER: TimeDelta = TimeDelta::minutes(30);

/// Finished or stopped sessions this old are folded into one summary line.
pub const FOLD_AFTER: TimeDelta = TimeDelta::days(2);

/// Rows that can be jumped to with a digit key.
const MAX_NUMBERED: u8 = 9;

/// What a row shows, in list order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shown {
    Waiting,
    Failed,
    Done,
    Working,
    Idle,
    /// The process is gone (or, across the boundary, silent for
    /// [`STALE_AFTER`]); the file outlived it (no `SessionEnd`).
    Stopped,
}

impl Shown {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "承認待ち",
            Self::Failed => "失敗",
            Self::Done => "完了",
            Self::Working => "作業中",
            Self::Idle => "待機",
            Self::Stopped => "止まっている",
        }
    }
}

/// Where this dashboard runs, to tell which sessions' pids it can check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub host: HostKind,
    pub hostname: Option<String>,
}

impl Side {
    /// Same machine and pid namespace as `s`.
    #[must_use]
    pub fn owns(&self, s: &SessionState) -> bool {
        s.host == self.host
            && match (&self.hostname, &s.hostname) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

/// One session, cleaned for display, with what the key actions need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Digit key for this row; `None` for stopped rows and past the ninth.
    pub number: Option<u8>,
    pub shown: Shown,
    pub session_id: String,
    pub host: HostKind,
    /// Pid namespace and machine match this dashboard's.
    pub local: bool,
    pub repo: String,
    pub project: String,
    /// Worktree directory name, or the branch for a main checkout.
    pub worktree: String,
    pub branch: Option<String>,
    /// `#S:#I.#P`, cleaned.
    pub tmux: Option<String>,
    /// `%N`, validated.
    pub tmux_pane: Option<String>,
    /// Raw paths for git and `-c` (never printed).
    pub repo_path: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub started_at: DateTime<FixedOffset>,
    pub updated_at: DateTime<FixedOffset>,
}

impl Row {
    /// Identity of the row's current state: a finished session counts as
    /// seen until it changes again.
    #[must_use]
    pub fn seen_key(&self) -> String {
        format!("{}@{}", self.session_id, self.updated_at.to_rfc3339())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Board {
    pub rows: Vec<Row>,
    /// Sessions folded away (finished or stopped for [`FOLD_AFTER`]).
    pub folded: usize,
    /// Index into `rows` of the one session to look at next.
    pub next: Option<usize>,
}

/// Build the board. `alive(pid)` answers for this side's pids (`None` when it
/// can't tell). `seen` holds [`Row::seen_key`]s of finished sessions already
/// visited from the dashboard. The state files are consumed: folded sessions
/// leave nothing behind but the count.
pub fn build<S: std::hash::BuildHasher>(
    sessions: Vec<SessionState>,
    now: DateTime<FixedOffset>,
    side: &Side,
    alive: &mut dyn FnMut(u32) -> Option<bool>,
    seen: &HashSet<String, S>,
) -> Board {
    let mut board = Board::default();
    for s in sessions {
        let (Ok(started_at), Ok(updated_at)) = (
            DateTime::parse_from_rfc3339(&s.started_at),
            DateTime::parse_from_rfc3339(&s.updated_at),
        ) else {
            continue;
        };
        let idle_for = now - updated_at;
        let local = side.owns(&s);
        // Without a usable pid answer (other side, no pid, or no `kill`),
        // silence is the only signal left.
        let stopped = match (local, s.pid) {
            (true, Some(pid)) => alive(pid).map_or(idle_for >= STALE_AFTER, |a| !a),
            _ => idle_for >= STALE_AFTER,
        };
        let shown = if stopped {
            Shown::Stopped
        } else {
            match s.status {
                Status::Waiting => Shown::Waiting,
                Status::Failed => Shown::Failed,
                Status::Done => Shown::Done,
                Status::Working => Shown::Working,
                Status::Idle => Shown::Idle,
            }
        };
        if matches!(shown, Shown::Done | Shown::Stopped) && idle_for >= FOLD_AFTER {
            board.folded += 1;
            continue;
        }
        board
            .rows
            .push(row(s, shown, local, started_at, updated_at));
    }

    // Blocked and finished sessions oldest first (they have waited longest);
    // running ones most recently active first.
    board.rows.sort_by(|a, b| {
        a.shown.cmp(&b.shown).then_with(|| {
            let by_time = if matches!(a.shown, Shown::Waiting | Shown::Failed | Shown::Done) {
                a.updated_at.cmp(&b.updated_at)
            } else {
                b.updated_at.cmp(&a.updated_at)
            };
            by_time.then_with(|| a.session_id.cmp(&b.session_id))
        })
    });
    let mut n = 0;
    for r in &mut board.rows {
        if r.shown != Shown::Stopped && n < MAX_NUMBERED {
            n += 1;
            r.number = Some(n);
        }
    }

    let first = |want: Shown, rows: &[Row]| {
        rows.iter()
            .position(|r| r.shown == want && !(want == Shown::Done && seen.contains(&r.seen_key())))
    };
    board.next = first(Shown::Waiting, &board.rows)
        .or_else(|| first(Shown::Failed, &board.rows))
        .or_else(|| first(Shown::Done, &board.rows))
        .or_else(|| first(Shown::Working, &board.rows));
    board
}

fn row(
    s: SessionState,
    shown: Shown,
    local: bool,
    started_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
) -> Row {
    let repo = s
        .repo
        .as_deref()
        .map_or_else(|| text::base_name(&s.cwd), text::base_name);
    let branch = s
        .branch
        .as_deref()
        .map(text::clean)
        .filter(|b| !b.is_empty());
    let worktree = match (&s.worktree, &s.repo) {
        (Some(w), Some(r)) if w != r => text::base_name(w),
        (Some(w), None) => text::base_name(w),
        _ => branch.clone().unwrap_or_else(|| "-".to_string()),
    };
    Row {
        number: None,
        shown,
        host: s.host,
        local,
        repo,
        project: s.project.as_deref().map(text::clean).unwrap_or_default(),
        worktree,
        branch,
        tmux: s.tmux.as_deref().map(text::clean).filter(|t| !t.is_empty()),
        tmux_pane: s.tmux_pane.filter(|p| crate::probe::is_pane_id(p)),
        repo_path: s.repo.map(PathBuf::from),
        worktree_path: s.worktree.map(PathBuf::from),
        session_id: s.session_id,
        started_at,
        updated_at,
    }
}

/// Compact age: `40s`, `12m`, `3h`, `6d`.
#[must_use]
pub fn age(d: TimeDelta) -> String {
    let s = d.num_seconds().max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctk::focus::SCHEMA;

    const NOW: &str = "2026-01-05T12:00:00.000+09:00";

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn ago(minutes: i64) -> String {
        (at(NOW) - TimeDelta::minutes(minutes)).to_rfc3339()
    }

    fn session(id: &str, status: Status, updated_min_ago: i64) -> SessionState {
        SessionState {
            schema: SCHEMA,
            session_id: id.into(),
            host: HostKind::Host,
            hostname: Some("example-host".into()),
            tmux: Some("work:2.1".into()),
            tmux_pane: Some("%3".into()),
            pid: Some(100),
            cwd: "/work/repo-a".into(),
            repo: Some("/work/repo-a".into()),
            worktree: Some("/work/repo-a/.claude/worktrees/feat-x".into()),
            branch: Some("feat/x".into()),
            project: Some("demo".into()),
            status,
            started_at: ago(600),
            updated_at: ago(updated_min_ago),
        }
    }

    fn side() -> Side {
        Side {
            host: HostKind::Host,
            hostname: Some("example-host".into()),
        }
    }

    fn build_with(sessions: Vec<SessionState>, dead: &[u32]) -> Board {
        let dead = dead.to_vec();
        build(
            sessions,
            at(NOW),
            &side(),
            &mut |pid| Some(!dead.contains(&pid)),
            &HashSet::new(),
        )
    }

    fn ids(b: &Board) -> Vec<&str> {
        b.rows.iter().map(|r| r.session_id.as_str()).collect()
    }

    #[test]
    fn orders_by_status_then_time_and_numbers_live_rows() {
        let mut dead = session("dead", Status::Working, 5);
        dead.pid = Some(7);
        let b = build_with(
            vec![
                session("work-old", Status::Working, 20),
                session("work-new", Status::Working, 1),
                session("done", Status::Done, 12),
                dead,
                session("wait-new", Status::Waiting, 1),
                session("wait-old", Status::Waiting, 9),
                session("fail", Status::Failed, 3),
                session("idle", Status::Idle, 2),
            ],
            &[7],
        );
        assert_eq!(
            ids(&b),
            [
                "wait-old", "wait-new", "fail", "done", "work-new", "work-old", "idle", "dead"
            ]
        );
        let numbers: Vec<_> = b.rows.iter().map(|r| r.number).collect();
        assert_eq!(numbers[..7], [1, 2, 3, 4, 5, 6, 7].map(Some));
        assert_eq!(numbers[7], None);
        assert_eq!(b.rows[7].shown, Shown::Stopped);
        assert_eq!(b.next, Some(0));
    }

    #[test]
    fn next_prefers_failed_then_unseen_done_then_working() {
        let b = build_with(
            vec![
                session("fail", Status::Failed, 3),
                session("done", Status::Done, 2),
            ],
            &[],
        );
        assert_eq!(b.rows[b.next.unwrap()].session_id, "fail");

        let sessions = || {
            vec![
                session("done", Status::Done, 2),
                session("work", Status::Working, 1),
            ]
        };
        let b = build_with(sessions(), &[]);
        assert_eq!(b.rows[b.next.unwrap()].session_id, "done");

        let seen: HashSet<String> = [b.rows[0].seen_key()].into();
        let b = build(sessions(), at(NOW), &side(), &mut |_| Some(true), &seen);
        assert_eq!(b.rows[b.next.unwrap()].session_id, "work");

        assert_eq!(
            build_with(vec![session("i", Status::Idle, 1)], &[]).next,
            None
        );
    }

    #[test]
    fn other_side_sessions_stop_after_thirty_minutes() {
        let mut fresh = session("fresh", Status::Waiting, 29);
        fresh.host = HostKind::Container;
        let mut stale = session("stale", Status::Waiting, 30);
        stale.host = HostKind::Container;
        // Never asked about container pids.
        let b = build(
            vec![fresh, stale],
            at(NOW),
            &side(),
            &mut |_| panic!("container pid checked"),
            &HashSet::new(),
        );
        let shown: Vec<_> = b
            .rows
            .iter()
            .map(|r| (r.session_id.as_str(), r.shown))
            .collect();
        assert_eq!(
            shown,
            [("fresh", Shown::Waiting), ("stale", Shown::Stopped)]
        );
        assert!(!b.rows[0].local);
    }

    #[test]
    fn other_hostname_is_the_other_side() {
        let mut s = session("s", Status::Working, 31);
        s.hostname = Some("other-host".into());
        let b = build(
            vec![s],
            at(NOW),
            &side(),
            &mut |_| panic!(),
            &HashSet::new(),
        );
        assert_eq!(b.rows[0].shown, Shown::Stopped);
    }

    #[test]
    fn unknown_liveness_falls_back_to_silence() {
        let b = build(
            vec![
                session("quiet", Status::Working, 30),
                session("recent", Status::Working, 29),
            ],
            at(NOW),
            &side(),
            &mut |_| None,
            &HashSet::new(),
        );
        let shown: Vec<_> = b
            .rows
            .iter()
            .map(|r| (r.session_id.as_str(), r.shown))
            .collect();
        assert_eq!(
            shown,
            [("recent", Shown::Working), ("quiet", Shown::Stopped)]
        );
    }

    #[test]
    fn folds_two_day_old_finished_and_stopped_sessions() {
        let two_days = 2 * 24 * 60;
        let mut dead = session("dead", Status::Waiting, two_days);
        dead.pid = Some(7);
        let b = build_with(
            vec![
                session("old-done", Status::Done, two_days),
                session("young-done", Status::Done, two_days - 1),
                dead,
                session("old-wait", Status::Waiting, two_days + 10),
            ],
            &[7],
        );
        assert_eq!(b.folded, 2);
        assert_eq!(ids(&b), ["old-wait", "young-done"]);
    }

    #[test]
    fn cleans_fields_and_labels_worktrees() {
        let mut s = session("s", Status::Working, 1);
        s.repo = Some("/work/re\u{202E}po-a".into());
        s.branch = Some("feat/\u{1b}[2Jx".into());
        s.tmux = Some("work\u{7}:2.1".into());
        s.tmux_pane = Some("%3; rm".into());
        s.project = Some("demo\u{200B}".into());
        let r = build_with(vec![s], &[]).rows.remove(0);
        assert_eq!(r.repo, "repo-a");
        assert_eq!(r.branch.as_deref(), Some("feat/[2Jx"));
        assert_eq!(r.tmux.as_deref(), Some("work:2.1"));
        assert_eq!(r.tmux_pane, None);
        assert_eq!(r.project, "demo");
        assert_eq!(r.worktree, "feat-x");

        let mut main = session("m", Status::Working, 1);
        main.worktree = main.repo.clone();
        assert_eq!(build_with(vec![main], &[]).rows[0].worktree, "feat/x");

        let mut outside = session("o", Status::Working, 1);
        outside.repo = None;
        outside.worktree = None;
        outside.branch = None;
        outside.cwd = "/tmp/scratch".into();
        let r = build_with(vec![outside], &[]).rows.remove(0);
        assert_eq!((r.repo.as_str(), r.worktree.as_str()), ("scratch", "-"));
    }

    #[test]
    fn unparsable_times_are_skipped() {
        let mut s = session("s", Status::Working, 1);
        s.updated_at = "yesterday".into();
        assert!(build_with(vec![s], &[]).rows.is_empty());
    }

    #[test]
    fn numbers_stop_at_nine() {
        let sessions = (0..11)
            .map(|i| session(&format!("s{i:02}"), Status::Working, i))
            .collect();
        let b = build_with(sessions, &[]);
        assert_eq!(b.rows[8].number, Some(9));
        assert_eq!(b.rows[9].number, None);
    }

    #[test]
    fn age_is_compact() {
        assert_eq!(age(TimeDelta::seconds(40)), "40s");
        assert_eq!(age(TimeDelta::minutes(12)), "12m");
        assert_eq!(age(TimeDelta::hours(3)), "3h");
        assert_eq!(age(TimeDelta::days(6)), "6d");
        assert_eq!(age(TimeDelta::seconds(-5)), "0s");
    }
}
