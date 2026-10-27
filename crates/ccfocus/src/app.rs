//! The live focus board shared by the `ccfocus` TUI and cctop's Focus panel:
//! what to re-read and when, the background git / gh work, and the keys.
//!
//! Call [`Focus::tick`] often (it decides what is due) and draw
//! [`Focus::view`]. Nothing read from the state files is kept between ticks
//! except the [`View`] built from them, so memory stays flat however long it
//! runs.

use crate::board::{self, Board, Shown, Side};
use crate::config::{self, Config};
use crate::events::{self, EventLog};
use crate::probe::{Probe, SystemProbe};
use crate::repos::{self, Job, Tasks, Worktree};
use crate::tmux::{self, Client};
use crate::ui::{TaskCount, Today, View};
use cctk::focus;
use chrono::{DateTime, Days, FixedOffset, Local, NaiveDate, TimeDelta, Timelike};
use crossterm::event::KeyCode;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// State files and today's events are re-read this often.
const SESSIONS_EVERY: Duration = Duration::from_secs(2);
/// A pid's liveness is re-checked this often.
const ALIVE_TTL: Duration = Duration::from_secs(10);
const WORKTREES_EVERY: Duration = Duration::from_secs(30);
/// gh results are cached this long (`r` does not bypass it).
const TASKS_EVERY: Duration = Duration::from_mins(10);
const PRUNE_EVERY: Duration = Duration::from_hours(1);
/// Key feedback stays this long.
const MESSAGE_FOR: Duration = Duration::from_secs(5);

#[derive(Debug)]
enum TaskState {
    Pending,
    Failed,
    Ready(Tasks),
}

pub struct Focus {
    state_dir: PathBuf,
    config: Config,
    warning: Option<String>,
    side: Side,
    client: Client,
    title: String,
    events: EventLog,
    alive: HashMap<u32, (Instant, Option<bool>)>,
    worktree_job: Job<Vec<Worktree>>,
    worktrees: Option<Vec<Worktree>>,
    /// Results carry the day they were fetched for.
    task_job: Job<(NaiveDate, Option<Tasks>)>,
    /// `repos::fetch_tasks`; replaced in tests so they never call gh.
    fetch_tasks: fn(&str, &[PathBuf]) -> Option<Tasks>,
    tasks: TaskState,
    tasks_day: Option<NaiveDate>,
    last_read: Option<Instant>,
    last_prune: Option<Instant>,
    board: Board,
    /// `Row::seen_key`s of finished sessions jumped to from here.
    seen: HashSet<String>,
    cursor: Option<String>,
    message: Option<(String, Instant)>,
    view: View,
}

impl Focus {
    /// A board over `state_dir`, using the environment (tmux client, host /
    /// container, hostname). Nothing is read until the first [`tick`](Self::tick).
    #[must_use]
    pub fn open(state_dir: PathBuf) -> Self {
        let (config, warning) = config::load(&state_dir);
        let probe = SystemProbe::new(Duration::from_secs(1));
        let side = Side {
            host: probe.host(),
            hostname: probe.hostname(),
        };
        let client = Client::from_env();
        let title = tmux::own_window(&client)
            .map_or_else(|| "focus".to_string(), |w| format!("focus · {w}"));
        Self::with(state_dir, config, warning, side, client, title)
    }

    fn with(
        state_dir: PathBuf,
        config: Config,
        warning: Option<String>,
        side: Side,
        client: Client,
        title: String,
    ) -> Self {
        let now = Local::now().fixed_offset();
        let view = View {
            now,
            title: title.clone(),
            parallel: 0,
            limit: config.limit,
            round: None,
            board: Board::default(),
            cursor: None,
            today: Today {
                tasks: TaskCount::Pending,
                switches: 0,
                yesterday_switches: 0,
                parallel_avg: 0.0,
                parallel_max: 0,
                week: [0; 7],
            },
            candidates: None,
            pr_known: false,
            message: None,
            warning: warning.clone(),
        };
        Self {
            events: EventLog::new(&state_dir),
            state_dir,
            config,
            warning,
            side,
            client,
            title,
            alive: HashMap::new(),
            worktree_job: Job::new(WORKTREES_EVERY),
            worktrees: None,
            task_job: Job::new(TASKS_EVERY),
            fetch_tasks: repos::fetch_tasks,
            tasks: TaskState::Pending,
            tasks_day: None,
            last_read: None,
            last_prune: None,
            board: Board::default(),
            seen: HashSet::new(),
            cursor: None,
            message: None,
            view,
        }
    }

    #[must_use]
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Do whatever is due. `force` re-reads everything now (and the config),
    /// except gh, which keeps its cache.
    pub fn tick(&mut self, force: bool) {
        self.tick_at(Local::now().fixed_offset(), Instant::now(), force);
    }

    fn tick_at(&mut self, now: DateTime<FixedOffset>, clock: Instant, force: bool) {
        let mut changed = false;
        if let Some(w) = self.worktree_job.poll() {
            self.worktrees = Some(w);
            changed = true;
        }
        let today = now.date_naive();
        if let Some((day, t)) = self.task_job.poll() {
            if day == today {
                self.tasks = t.map_or(TaskState::Failed, TaskState::Ready);
            } else {
                // Fetched for a day that has ended: fetch again for this one.
                self.tasks = TaskState::Pending;
                self.tasks_day = None;
            }
            changed = true;
        }
        if self
            .message
            .as_ref()
            .is_some_and(|(_, at)| clock.duration_since(*at) >= MESSAGE_FOR)
        {
            self.message = None;
            changed = true;
        }

        let due = force
            || self
                .last_read
                .is_none_or(|t| clock.duration_since(t) >= SESSIONS_EVERY);
        if due {
            self.last_read = Some(clock);
            if force {
                (self.config, self.warning) = config::load(&self.state_dir);
            }
            if self
                .last_prune
                .is_none_or(|t| clock.duration_since(t) >= PRUNE_EVERY)
            {
                self.last_prune = Some(clock);
                let _ = crate::prune::prune(&self.state_dir, today, SystemTime::now());
            }
            self.events.refresh(today);
            self.read_board(now, clock);

            let repos = repos::local_repos(&self.board.rows);
            let scan = repos.clone();
            let _ = self.worktree_job.kick(clock, force, move || {
                scan.iter().flat_map(|r| repos::scan_repo(r)).collect()
            });
            let new_day = self.tasks_day != Some(today);
            if new_day {
                self.tasks = TaskState::Pending;
            }
            let since = start_of_day(now);
            let fetch = self.fetch_tasks;
            let started = self
                .task_job
                .kick(clock, new_day, move || (today, fetch(&since, &repos)));
            // A fetch still running at midnight ignores the kick; the next
            // tick tries again.
            if started {
                self.tasks_day = Some(today);
            }
            changed = true;
        }
        if changed {
            self.rebuild(now);
        }
    }

    fn read_board(&mut self, now: DateTime<FixedOffset>, clock: Instant) {
        let sessions = focus::list_sessions(&self.state_dir);
        let cache = &mut self.alive;
        let mut used = HashSet::new();
        let mut alive = |pid: u32| {
            used.insert(pid);
            match cache.get(&pid) {
                Some((at, v)) if clock.duration_since(*at) < ALIVE_TTL => *v,
                _ => {
                    let v = pid_alive(pid);
                    cache.insert(pid, (clock, v));
                    v
                }
            }
        };
        self.board = board::build(sessions, now, &self.side, &mut alive, &self.seen);
        self.alive.retain(|pid, _| used.contains(pid));
        let live: HashSet<String> = self.board.rows.iter().map(board::Row::seen_key).collect();
        self.seen.retain(|k| live.contains(k));
    }

    fn rebuild(&mut self, now: DateTime<FixedOffset>) {
        let today = now.date_naive();
        let day = |back: u64| {
            self.events
                .day(today.checked_sub_days(Days::new(back)).unwrap_or(today))
        };
        let stats = day(0);
        let mut week = [0u32; 7];
        for (i, slot) in week.iter_mut().enumerate() {
            *slot = day(6 - i as u64).switches;
        }
        let (tasks, heads) = match &self.tasks {
            TaskState::Pending => (TaskCount::Pending, None),
            TaskState::Failed => (TaskCount::Unknown, None),
            TaskState::Ready(t) => (TaskCount::Known(t.done), Some(&t.heads)),
        };
        let candidates = self
            .worktrees
            .as_ref()
            .map(|w| repos::candidates(w, heads, &self.board.rows));
        let pr_known = heads.is_some_and(|h| {
            repos::local_repos(&self.board.rows)
                .iter()
                .all(|r| h.contains_key(r))
        });

        let cursor = self
            .cursor
            .as_ref()
            .and_then(|id| self.board.rows.iter().position(|r| &r.session_id == id))
            .or(self.board.next);

        self.view = View {
            now,
            title: self.title.clone(),
            parallel: self
                .events
                .active_since(now - TimeDelta::seconds(events::WINDOW_SECS)),
            limit: self.config.limit,
            round: next_round(now, self.config.round_minutes),
            board: self.board.clone(),
            cursor,
            today: Today {
                tasks,
                switches: stats.switches,
                yesterday_switches: day(1).switches,
                parallel_avg: stats.parallel_avg,
                parallel_max: stats.parallel_max,
                week,
            },
            candidates,
            pr_known,
            message: self.message.as_ref().map(|(m, _)| m.clone()),
            warning: self.warning.clone(),
        };
    }

    fn say(&mut self, msg: String) {
        self.message = Some((msg, Instant::now()));
        self.rebuild(self.view.now);
    }

    /// Handle a key. `true` when the board should close (`q` / `Esc`).
    pub fn on_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char(c @ '1'..='9') => {
                let n = u8::try_from(c.to_digit(10).unwrap_or(0)).unwrap_or(0);
                match self.board.rows.iter().position(|r| r.number == Some(n)) {
                    Some(i) => self.jump(i),
                    None => self.say(format!("no session [{n}]")),
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Enter => {
                if let Some(i) = self.view.cursor {
                    self.jump(i);
                }
            }
            KeyCode::Char('d') => self.on_cursor(|row, client| {
                let base = row.worktree_path.as_deref().and_then(repos::merge_base);
                tmux::diff(row, client, base.as_deref())
            }),
            KeyCode::Char('p') => self.on_cursor(tmux::rename),
            KeyCode::Char('r') => self.tick(true),
            _ => {}
        }
        false
    }

    fn move_cursor(&mut self, by: isize) {
        let n = self.board.rows.len();
        if n == 0 {
            return;
        }
        let at = self.view.cursor.unwrap_or(0);
        let to = at.saturating_add_signed(by).min(n - 1);
        self.cursor = Some(self.board.rows[to].session_id.clone());
        self.rebuild(self.view.now);
    }

    fn on_cursor(&mut self, plan: impl FnOnce(&board::Row, &Client) -> tmux::Plan) {
        let Some(row) = self.view.cursor.and_then(|i| self.board.rows.get(i)) else {
            return;
        };
        match plan(row, &self.client).and_then(|a| tmux::run(&a)) {
            Ok(()) => {}
            Err(e) => self.say(e),
        }
    }

    fn jump(&mut self, i: usize) {
        let Some(row) = self.board.rows.get(i) else {
            return;
        };
        let done = (row.shown == Shown::Done).then(|| row.seen_key());
        match tmux::jump(row, &self.client, &self.config).and_then(|a| tmux::run(&a)) {
            Ok(()) => {
                // A finished session counts as seen once visited: pick the
                // next one without it.
                if let Some(key) = done {
                    self.seen.insert(key);
                    let now = self.view.now;
                    self.read_board(now, Instant::now());
                    self.rebuild(now);
                }
            }
            Err(e) => self.say(e),
        }
    }
}

/// `YYYY-MM-DDT00:00:00+HH:MM` for `now`'s day.
fn start_of_day(now: DateTime<FixedOffset>) -> String {
    format!("{}T00:00:00{}", now.format("%Y-%m-%d"), now.format("%:z"))
}

/// The next check-in on a grid of `minutes` from local midnight, and the
/// minutes left until it (rounded up). `None` when `minutes` is 0.
#[must_use]
pub fn next_round(now: DateTime<FixedOffset>, minutes: u32) -> Option<(String, i64)> {
    if minutes == 0 {
        return None;
    }
    let step = i64::from(minutes) * 60;
    let since_midnight = i64::from(now.num_seconds_from_midnight());
    let next = (since_midnight / step + 1) * step;
    let left = (next - since_midnight + 59) / 60;
    let at = next % 86_400;
    Some((format!("{:02}:{:02}", at / 3600, at % 3600 / 60), left))
}

/// Whether `pid` is running, via `kill -0` (`std` has no portable check and
/// this workspace forbids `unsafe`). A permission error still means the
/// process exists. `None` when the answer is unclear.
#[must_use]
pub fn pid_alive(pid: u32) -> Option<bool> {
    if cfg!(not(unix)) || pid == 0 {
        return None;
    }
    let mut cmd = std::process::Command::new("kill");
    cmd.args(["-0", &pid.to_string()]).env("LC_ALL", "C");
    let out = crate::cmd::run(cmd, Duration::from_secs(1))?;
    if out.success {
        return Some(true);
    }
    let err = out.stderr.to_ascii_lowercase();
    if err.contains("no such process") {
        Some(false)
    } else if err.contains("not permitted") {
        Some(true)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctk::focus::{HostKind, SCHEMA, SessionState, Status};
    use std::path::Path;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn rounds_land_on_the_grid() {
        assert_eq!(
            next_round(at("2026-01-05T14:24:00+09:00"), 30),
            Some(("14:30".into(), 6))
        );
        assert_eq!(
            next_round(at("2026-01-05T14:30:00+09:00"), 30),
            Some(("15:00".into(), 30))
        );
        assert_eq!(
            next_round(at("2026-01-05T14:29:30+09:00"), 30),
            Some(("14:30".into(), 1))
        );
        assert_eq!(
            next_round(at("2026-01-05T23:45:00+09:00"), 60),
            Some(("00:00".into(), 15))
        );
        assert_eq!(next_round(at("2026-01-05T14:24:00+09:00"), 0), None);
    }

    #[test]
    fn start_of_day_keeps_the_offset() {
        assert_eq!(
            start_of_day(at("2026-01-05T14:24:00+09:00")),
            "2026-01-05T00:00:00+09:00"
        );
    }

    #[cfg(unix)]
    #[test]
    fn own_pid_is_alive() {
        // The sandbox some runs use refuses signals; that still reads as alive.
        assert_eq!(pid_alive(std::process::id()), Some(true));
        assert_eq!(pid_alive(0), None);
    }

    fn write_session(dir: &Path, id: &str, status: Status, updated: &str) {
        let s = SessionState {
            schema: SCHEMA,
            session_id: id.into(),
            host: HostKind::Container,
            hostname: Some("example-container".into()),
            tmux: None,
            tmux_pane: None,
            pid: Some(1),
            cwd: "/work/none".into(),
            repo: None,
            worktree: None,
            branch: None,
            project: Some("demo".into()),
            status,
            started_at: "2026-01-05T09:00:00+09:00".into(),
            updated_at: updated.into(),
        };
        let sessions = focus::sessions_dir(dir);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(focus::session_file_name(id)),
            serde_json::to_string(&s).unwrap(),
        )
        .unwrap();
    }

    fn board_over(dir: &Path) -> Focus {
        let mut f = Focus::with(
            dir.to_path_buf(),
            Config::default(),
            None,
            Side {
                host: HostKind::Host,
                hostname: Some("example-host".into()),
            },
            Client::default(),
            "focus".into(),
        );
        f.fetch_tasks = |_, _| None;
        f
    }

    #[test]
    fn tick_reads_sessions_events_and_keys_move_the_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "00000000-0000-4000-8000-00000000000a";
        let b = "00000000-0000-4000-8000-00000000000b";
        write_session(tmp.path(), a, Status::Waiting, "2026-01-05T14:20:00+09:00");
        write_session(tmp.path(), b, Status::Working, "2026-01-05T14:23:00+09:00");
        let events = focus::events_dir(tmp.path());
        std::fs::create_dir_all(&events).unwrap();
        std::fs::write(
            events.join("20260105.tsv"),
            format!(
                "2026-01-05T14:00:00.000+09:00\t{a}\tUserPromptSubmit\n2026-01-05T14:10:00.000+09:00\t{b}\tUserPromptSubmit\n"
            ),
        )
        .unwrap();

        let mut f = board_over(tmp.path());
        let now = at("2026-01-05T14:24:00+09:00");
        f.tick_at(now, Instant::now(), false);
        let v = f.view();
        assert_eq!(v.board.rows.len(), 2);
        assert_eq!(v.board.next, Some(0));
        assert_eq!(v.cursor, Some(0));
        assert_eq!(v.parallel, 2);
        assert_eq!(v.today.switches, 1);
        assert_eq!(v.round, Some(("14:30".into(), 6)));

        assert!(!f.on_key(KeyCode::Char('j')));
        assert_eq!(f.view().cursor, Some(1));
        f.on_key(KeyCode::Char('j'));
        assert_eq!(f.view().cursor, Some(1), "stops at the last row");
        f.on_key(KeyCode::Up);
        assert_eq!(f.view().cursor, Some(0));

        // Outside tmux the jump explains itself instead of running anything.
        f.on_key(KeyCode::Char('1'));
        assert!(f.view().message.as_deref().unwrap().contains("tmux"));
        f.on_key(KeyCode::Char('7'));
        assert_eq!(f.view().message.as_deref(), Some("no session [7]"));
        assert!(f.on_key(KeyCode::Char('q')));
    }

    #[test]
    fn a_broken_config_is_reported_and_reloaded_on_r() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join(config::FILE_NAME), "limit = x\n").unwrap();
        let (config, warning) = config::load(tmp.path());
        let mut f = Focus::with(
            tmp.path().to_path_buf(),
            config,
            warning,
            Side {
                host: HostKind::Host,
                hostname: None,
            },
            Client::default(),
            "focus".into(),
        );
        f.fetch_tasks = |_, _| None;
        f.tick_at(at("2026-01-05T14:24:00+09:00"), Instant::now(), false);
        assert!(f.view().warning.is_some());
        std::fs::write(tmp.path().join(config::FILE_NAME), "limit = 2\n").unwrap();
        f.tick_at(at("2026-01-05T14:24:01+09:00"), Instant::now(), true);
        assert_eq!(f.view().warning, None);
        assert_eq!(f.view().limit, 2);
    }
}
