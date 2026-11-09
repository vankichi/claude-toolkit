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
/// Shown while a jump runs; cleared when it lands as planned.
const SWITCHING: &str = "switching…";
/// A jump key pressed while one runs.
const STILL_SWITCHING: &str = "still switching to the last session";

/// The jump's own notes: kept while it runs, cleared when it lands.
fn is_jump_note(m: &str) -> bool {
    m == SWITCHING || m == STILL_SWITCHING
}

/// What a jump's thread sends back: the finished row's `seen_key` (to mark
/// it seen on arrival) and [`tmux::run_jump`]'s result.
type JumpDone = (Option<String>, Result<Option<String>, String>);

#[derive(Debug)]
enum TaskState {
    Pending,
    Failed,
    Ready(Tasks),
}

pub struct Focus {
    state_dir: PathBuf,
    /// `$CCFOCUS_CONTAINER_TMUX` as read at start-up, reused on reload; unset
    /// in tests so they never depend on the environment.
    container_tmux_env: config::Env,
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
    /// A jump runs here so a slow `container_tmux` or tmux never freezes the
    /// board; one at a time.
    jump_job: Job<JumpDone>,
    /// `tmux::run_jump`; replaced in tests so they never call tmux.
    run_jump: fn(&tmux::Jump) -> Result<Option<String>, String>,
    /// `cmd::stop_running`; replaced in tests, where it would stop every
    /// other test's commands.
    stop_commands: fn(),
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
        let env = config::env();
        let (config, warning) = config::load_with(&state_dir, &env);
        let probe = SystemProbe::new(Duration::from_secs(1));
        let side = Side {
            host: probe.host(),
            hostname: probe.hostname(),
        };
        let client = Client::from_env();
        let title = tmux::own_window(&client)
            .map_or_else(|| "focus".to_string(), |w| format!("focus · {w}"));
        let mut focus = Self::with(state_dir, config, warning, side, client, title);
        focus.container_tmux_env = env;
        focus
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
            container_tmux_env: Ok(None),
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
            jump_job: Job::new(Duration::ZERO),
            run_jump: tmux::run_jump,
            stop_commands: crate::cmd::stop_running,
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
        if let Some((done, result)) = self.jump_job.poll() {
            self.arrived(done, result, now, clock);
            changed = true;
        }
        // A jump's own note stays while it runs, however long that is.
        let jumping = self.jump_job.is_pending();
        if self.message.as_ref().is_some_and(|(m, at)| {
            clock.duration_since(*at) >= MESSAGE_FOR && !(jumping && is_jump_note(m))
        }) {
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
                (self.config, self.warning) =
                    config::load_with(&self.state_dir, &self.container_tmux_env);
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
        self.note(msg, Instant::now());
        self.rebuild(self.view.now);
    }

    /// Set the bottom line without rebuilding (for callers that rebuild
    /// anyway, like `tick_at`).
    fn note(&mut self, msg: String, clock: Instant) {
        self.message = Some((msg, clock));
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
            KeyCode::Char('d') => self.on_cursor(
                |row, client| {
                    let base = row.worktree_path.as_deref().and_then(repos::merge_base);
                    tmux::diff(row, client, base.as_deref())
                },
                tmux::open,
            ),
            KeyCode::Char('p') => self.on_cursor(tmux::rename, tmux::run),
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

    fn on_cursor(
        &mut self,
        plan: impl FnOnce(&board::Row, &Client) -> tmux::Plan,
        exec: fn(&[String]) -> Result<(), String>,
    ) {
        let Some(row) = self.view.cursor.and_then(|i| self.board.rows.get(i)) else {
            return;
        };
        match plan(row, &self.client).and_then(|a| exec(&a)) {
            Ok(()) => {}
            Err(e) => self.say(e),
        }
    }

    fn jump(&mut self, i: usize) {
        let Some(row) = self.board.rows.get(i) else {
            return;
        };
        // Refuse before planning, so a row that can't be planned doesn't
        // replace the note of the jump still running.
        if self.jump_job.is_pending() {
            return self.say(STILL_SWITCHING.into());
        }
        let done = (row.shown == Shown::Done).then(|| row.seen_key());
        let plan = match tmux::jump(row, &self.client, &self.config) {
            Ok(plan) => plan,
            Err(e) => return self.say(e),
        };
        // The result arrives through `tick`. A key pressed meanwhile is
        // refused rather than queued: the client may already be on its way.
        let run = self.run_jump;
        if self
            .jump_job
            .kick(Instant::now(), true, move || (done, run(&plan)))
        {
            self.say(SWITCHING.into());
        } else {
            self.say(STILL_SWITCHING.into());
        }
    }

    /// Call once when the board closes, before tearing the screen down.
    ///
    /// First wait for a jump still running, so a board closed right after a
    /// jump key doesn't drop the switch (the process exiting would end its
    /// thread mid-way); bounded by the jump's own timeouts. Then kill what
    /// the background jobs still run (git, gh): their timeouts live on
    /// threads that end with the process, so nothing else would stop them.
    pub fn close(&mut self) {
        let _ = self.jump_job.wait();
        (self.stop_commands)();
    }

    /// A jump's thread finished.
    fn arrived(
        &mut self,
        done: Option<String>,
        result: Result<Option<String>, String>,
        now: DateTime<FixedOffset>,
        clock: Instant,
    ) {
        match result {
            Ok(warning) => {
                // Arrived, but not as planned (a container step failed, or
                // a fallback target was used): say why. Otherwise drop the
                // "switching" note, unless a newer message replaced it.
                if let Some(w) = warning {
                    self.note(w, clock);
                } else if self.message.as_ref().is_some_and(|(m, _)| is_jump_note(m)) {
                    self.message = None;
                }
                // A finished session counts as seen once visited: pick the
                // next one without it.
                if let Some(key) = done {
                    self.seen.insert(key);
                    self.read_board(now, clock);
                }
            }
            Err(e) => self.note(e, clock),
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
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Set by the fakes in `closing_the_board_waits_for_a_jump_then_stops_commands`.
    static SWITCHED: AtomicBool = AtomicBool::new(false);
    static STOPPED_AFTER_SWITCH: AtomicBool = AtomicBool::new(false);
    /// Holds the fake jump in `the_jump_notes_outlast_the_message_timeout_while_it_runs`.
    static RELEASE: AtomicBool = AtomicBool::new(false);
    /// Holds the fake jump in `a_key_during_a_jump_is_refused_before_planning`.
    static HOLD_RELEASE: AtomicBool = AtomicBool::new(false);

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
            host_tmux_pane: None,
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
        f.run_jump = |_| Err("tests never run tmux".into());
        f.stop_commands = || {};
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

    /// Tick until the jump's result lands (it runs on its own thread).
    fn tick_until_arrived(f: &mut Focus, now: DateTime<FixedOffset>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while f.jump_job.is_pending() {
            assert!(Instant::now() < deadline, "the jump never finished");
            std::thread::sleep(Duration::from_millis(10));
            f.tick_at(now, Instant::now(), false);
        }
    }

    #[test]
    fn a_jump_runs_off_the_board_and_reports_when_it_lands() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "00000000-0000-4000-8000-00000000000a";
        write_session(tmp.path(), a, Status::Done, "2026-01-05T14:20:00+09:00");
        let mut f = board_over(tmp.path());
        f.client = Client {
            inside: true,
            pane: None,
        };
        f.run_jump = |_| Ok(Some("fell back to dev: no such pane".into()));
        let now = at("2026-01-05T14:24:00+09:00");
        f.tick_at(now, Instant::now(), false);
        assert_eq!(f.view().board.next, Some(0));

        // Whenever the thread finishes, its result waits for a tick, so
        // these hold however fast the fake is.
        f.on_key(KeyCode::Char('1'));
        assert_eq!(
            f.view().message.as_deref(),
            Some(SWITCHING),
            "the key doesn't wait for the jump"
        );
        f.on_key(KeyCode::Char('1'));
        assert_eq!(f.view().message.as_deref(), Some(STILL_SWITCHING));

        tick_until_arrived(&mut f, now);
        assert_eq!(
            f.view().message.as_deref(),
            Some("fell back to dev: no such pane")
        );
        assert_eq!(
            f.view().board.next,
            None,
            "the finished session counts as seen"
        );
    }

    #[test]
    fn a_jump_that_lands_as_planned_clears_only_its_own_note() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "00000000-0000-4000-8000-00000000000a";
        write_session(tmp.path(), a, Status::Waiting, "2026-01-05T14:20:00+09:00");
        let mut f = board_over(tmp.path());
        f.client = Client {
            inside: true,
            pane: None,
        };
        f.run_jump = |_| Ok(None);
        let now = at("2026-01-05T14:24:00+09:00");
        f.tick_at(now, Instant::now(), false);

        f.on_key(KeyCode::Char('1'));
        tick_until_arrived(&mut f, now);
        assert_eq!(f.view().message, None);

        f.on_key(KeyCode::Char('1'));
        f.on_key(KeyCode::Char('7'));
        tick_until_arrived(&mut f, now);
        assert_eq!(f.view().message.as_deref(), Some("no session [7]"));
    }

    #[test]
    fn closing_the_board_waits_for_a_jump_then_stops_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "00000000-0000-4000-8000-00000000000a";
        write_session(tmp.path(), a, Status::Waiting, "2026-01-05T14:20:00+09:00");
        let mut f = board_over(tmp.path());
        f.client = Client {
            inside: true,
            pane: None,
        };
        f.run_jump = |_| {
            std::thread::sleep(Duration::from_millis(200));
            SWITCHED.store(true, Ordering::SeqCst);
            Ok(None)
        };
        // Stopping commands first would kill the jump's own tmux.
        f.stop_commands =
            || STOPPED_AFTER_SWITCH.store(SWITCHED.load(Ordering::SeqCst), Ordering::SeqCst);
        f.tick_at(at("2026-01-05T14:24:00+09:00"), Instant::now(), false);

        f.on_key(KeyCode::Char('1'));
        assert!(f.on_key(KeyCode::Char('q')));
        f.close();
        assert!(SWITCHED.load(Ordering::SeqCst), "the switch ran to the end");
        assert!(
            STOPPED_AFTER_SWITCH.load(Ordering::SeqCst),
            "commands stopped after it"
        );
    }

    #[test]
    fn the_jump_notes_outlast_the_message_timeout_while_it_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "00000000-0000-4000-8000-00000000000a";
        write_session(tmp.path(), a, Status::Waiting, "2026-01-05T14:20:00+09:00");
        let mut f = board_over(tmp.path());
        f.client = Client {
            inside: true,
            pane: None,
        };
        f.run_jump = |_| {
            while !RELEASE.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None)
        };
        let now = at("2026-01-05T14:24:00+09:00");
        f.tick_at(now, Instant::now(), false);

        f.on_key(KeyCode::Char('1'));
        f.on_key(KeyCode::Char('1'));
        // The jump is held, so a tick past MESSAGE_FOR can't see a result.
        let later = Instant::now() + MESSAGE_FOR + Duration::from_secs(1);
        f.tick_at(now, later, false);
        assert_eq!(f.view().message.as_deref(), Some(STILL_SWITCHING));

        // Landing as planned clears the refusal note too.
        RELEASE.store(true, Ordering::SeqCst);
        tick_until_arrived(&mut f, now);
        assert_eq!(f.view().message, None);
    }

    #[test]
    fn a_key_during_a_jump_is_refused_before_planning() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "00000000-0000-4000-8000-00000000000a";
        write_session(tmp.path(), a, Status::Waiting, "2026-01-05T14:20:00+09:00");
        let mut f = board_over(tmp.path());
        f.client = Client {
            inside: true,
            pane: None,
        };
        f.run_jump = |_| {
            while !HOLD_RELEASE.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None)
        };
        let now = at("2026-01-05T14:24:00+09:00");
        f.tick_at(now, Instant::now(), false);

        f.on_key(KeyCode::Char('1'));
        // A key that couldn't be planned now (outside tmux) still reads as
        // a refusal, so the note keeps saying a jump is under way.
        f.client.inside = false;
        f.on_key(KeyCode::Char('1'));
        assert_eq!(f.view().message.as_deref(), Some(STILL_SWITCHING));

        HOLD_RELEASE.store(true, Ordering::SeqCst);
        tick_until_arrived(&mut f, now);
        assert_eq!(f.view().message, None);
    }

    #[test]
    fn a_failed_jump_says_why_and_leaves_the_session_unseen() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "00000000-0000-4000-8000-00000000000a";
        write_session(tmp.path(), a, Status::Done, "2026-01-05T14:20:00+09:00");
        let mut f = board_over(tmp.path());
        f.client = Client {
            inside: true,
            pane: None,
        };
        f.run_jump = |_| Err("can't find session: dev".into());
        let now = at("2026-01-05T14:24:00+09:00");
        f.tick_at(now, Instant::now(), false);

        f.on_key(KeyCode::Enter);
        tick_until_arrived(&mut f, now);
        assert_eq!(f.view().message.as_deref(), Some("can't find session: dev"));
        assert_eq!(f.view().board.next, Some(0));
        // The next key starts a new jump.
        f.on_key(KeyCode::Enter);
        assert!(f.jump_job.is_pending());
    }

    #[test]
    fn a_broken_config_is_reported_and_reloaded_on_r() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join(config::FILE_NAME), "limit = x\n").unwrap();
        let (config, warning) = config::load_with(tmp.path(), &Ok(None));
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
