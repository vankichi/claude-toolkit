//! Async runtime: owns the terminal, tails every currently-active session,
//! periodically rescans the stats/map subsystems, and multiplexes tail events
//! / render ticks / keyboard input in a single `tokio::select!` loop.
//!
//! Rendering and key dispatch branch on [`App::mode`]: the overview draws all
//! four panels, while a drilled-in panel renders and drives the corresponding
//! full view (`ccstat`/`ccmap` re-entrant API, or the self-rendered Now
//! detail). The Now panel rolls up all active sessions (see [`crate::now`]).

use crate::app::{Action, App, Mode, Panel};
use crate::now::NowStats;
use crate::overview;
use crate::store::Dashboard;
use anyhow::Result;
use ccmap::discover::Context;
use ccstat::scan::ScanConfig;
use cctk::jsonl::Line;
use chrono::Utc;
use crossterm::event::{EventStream, KeyCode};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Runtime configuration resolved from CLI flags.
pub struct RunConfig {
    pub projects_dir: PathBuf,
    pub claude_dir: PathBuf,
    pub project_dir: PathBuf,
    /// ccfocus state directory for the Focus panel.
    pub focus_dir: PathBuf,
    pub refresh: Duration,
    pub rescan_every: Duration,
}

const TAIL_CHANNEL_CAPACITY: usize = 1024;
/// A session whose log was modified within this window counts as "active" and
/// is tailed into the Now rollup. Matches the rate chart's 15-minute window so
/// tailed sessions always have data to plot.
const ACTIVE_WINDOW: Duration = Duration::from_mins(15);

impl RunConfig {
    fn scan_cfg(&self) -> ScanConfig {
        ScanConfig {
            projects_dir: self.projects_dir.clone(),
        }
    }

    fn ctx(&self) -> Context {
        Context {
            claude_dir: self.claude_dir.clone(),
            project_dir: self.project_dir.clone(),
        }
    }
}

/// The live session tails feeding the Now rollup: each tail's channel tag
/// and abortable task, keyed by log path.
struct TailSet {
    tx: mpsc::Sender<(usize, Line)>,
    tails: HashMap<PathBuf, (usize, JoinHandle<Result<()>>)>,
    next_tag: usize,
}

/// Entry point: build the store, init the terminal, run the loop, restore.
pub async fn run(cfg: RunConfig) -> Result<()> {
    let today = Utc::now().date_naive();

    let mut dash = Dashboard::load(&cfg.scan_cfg(), &cfg.ctx(), today, cfg.focus_dir.clone());
    dash.focus.tick(true);
    let mut app = App::new();

    let (tx, rx) = mpsc::channel::<(usize, Line)>(TAIL_CHANNEL_CAPACITY);
    let mut tails = TailSet {
        tx,
        tails: HashMap::new(),
        next_tag: 0,
    };
    tails.reconcile(&cfg.projects_dir, &mut dash.now);
    dash.now.apply_names(&crate::agents::names().await);

    let mut terminal = cctk::term::Tui::init()?;
    let result = run_loop(&mut terminal, &cfg, &mut dash, &mut app, rx, &mut tails).await;
    // A jump started in the Focus panel: let the switch finish before exit,
    // then stop the Focus panel's git / gh still running.
    dash.focus.close();
    drop(terminal);
    tails.abort_all();
    result
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    cfg: &RunConfig,
    dash: &mut Dashboard,
    app: &mut App,
    mut rx: mpsc::Receiver<(usize, Line)>,
    tails: &mut TailSet,
) -> Result<()> {
    let scan_cfg = cfg.scan_cfg();
    let ctx = cfg.ctx();
    let mut events = EventStream::new();
    let mut render_tick = tokio::time::interval(cfg.refresh);
    render_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut rescan_tick = tokio::time::interval(cfg.rescan_every);
    rescan_tick.reset(); // don't fire immediately; we just loaded
    rescan_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        draw(terminal, dash, app)?;

        tokio::select! {
            maybe = rx.recv() => {
                if let Some((tag, line)) = maybe {
                    dash.now.ingest(tag, &line);
                }
            }
            _ = render_tick.tick() => dash.focus.tick(false),
            _ = rescan_tick.tick() => {
                let today = Utc::now().date_naive();
                dash.rescan(&scan_cfg, &ctx, today);
                tails.reconcile(&cfg.projects_dir, &mut dash.now);
                dash.now.apply_names(&crate::agents::names().await);
            }
            maybe_ev = events.next() => {
                let Some(Ok(ev)) = maybe_ev else { continue };
                if let Some(code) = cctk::term::pressed(&ev)
                    && handle_key(code, terminal, app, dash, &scan_cfg, &ctx)?
                {
                    return Ok(());
                }
            }
        }
    }
}

/// Render the active view per mode.
fn draw(terminal: &mut DefaultTerminal, dash: &mut Dashboard, app: &App) -> Result<()> {
    match app.mode {
        Mode::Overview => {
            // Reflect the overview filter into the reused sub-view state so the
            // compact panels (and any subsequent drill-down) share it.
            dash.stats.set_filter(app.filter.clone());
            dash.map.set_filter(app.filter.clone());
            terminal.draw(|f| overview::draw(f, dash, app))?;
        }
        Mode::Drill(Panel::Now) => {
            terminal.draw(|f| overview::draw_now_detail(f, &dash.now))?;
        }
        Mode::Drill(Panel::Stats) => {
            terminal.draw(|f| ccstat::ui::draw(f, &dash.stats))?;
        }
        Mode::Drill(Panel::Map) => {
            terminal.draw(|f| ccmap::ui::draw(f, &dash.map))?;
        }
        Mode::Drill(Panel::Focus) => {
            terminal.draw(|f| ccfocus::ui::draw(f, f.area(), dash.focus.view()))?;
        }
    }
    Ok(())
}

/// Dispatch a keypress by mode. Returns `true` when the app should quit.
fn handle_key(
    code: KeyCode,
    terminal: &mut DefaultTerminal,
    app: &mut App,
    dash: &mut Dashboard,
    scan_cfg: &ScanConfig,
    ctx: &Context,
) -> Result<bool> {
    match app.mode {
        Mode::Overview => match app.on_overview_key(code) {
            Action::Quit => return Ok(true),
            Action::EnterDrill(_) => app.enter_drill(),
            Action::None => {}
        },
        Mode::Drill(Panel::Now) => {
            if matches!(code, KeyCode::Char('q') | KeyCode::Esc) {
                app.exit_drill();
            }
        }
        Mode::Drill(Panel::Stats) => {
            if ccstat::ui::handle_key(code, &mut dash.stats, scan_cfg, ctx) {
                app.exit_drill();
            }
        }
        Mode::Drill(Panel::Map) => {
            if ccmap::ui::handle_key(code, &mut dash.map, ctx, terminal)? {
                app.exit_drill();
            }
        }
        Mode::Drill(Panel::Focus) => {
            if dash.focus.on_key(code) {
                app.exit_drill();
            }
        }
    }
    Ok(false)
}

/// Session logs modified within [`ACTIVE_WINDOW`] — the sessions to tail now.
fn active_sessions(projects_dir: &Path) -> Vec<PathBuf> {
    let cutoff = SystemTime::now().checked_sub(ACTIVE_WINDOW);
    cctk::paths::session_files(projects_dir)
        .into_iter()
        .filter(
            |p| match (std::fs::metadata(p).and_then(|m| m.modified()), cutoff) {
                (Ok(mtime), Some(c)) => mtime >= c,
                // If mtime or the cutoff is unavailable, err toward including it.
                _ => true,
            },
        )
        .collect()
}

impl TailSet {
    /// Reconcile the live tails against the currently-active sessions: spawn
    /// a tail for each newly-active session and abort (and forget) those that
    /// went idle, so the Now rollup tracks exactly the active set.
    fn reconcile(&mut self, projects_dir: &Path, now: &mut NowStats) {
        let desired: Vec<PathBuf> = active_sessions(projects_dir);

        // Drop tails whose session is no longer active.
        let gone: Vec<PathBuf> = self
            .tails
            .keys()
            .filter(|p| !desired.contains(p))
            .cloned()
            .collect();
        for path in gone {
            if let Some((tag, handle)) = self.tails.remove(&path) {
                handle.abort();
                now.drop_session(tag);
            }
        }

        // Spawn tails for newly-active sessions.
        for path in desired {
            if self.tails.contains_key(&path) {
                continue;
            }
            let tag = self.next_tag;
            self.next_tag += 1;
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            now.register_session(tag, id);
            let handle = cctk::tail::spawn(path.clone(), tag, self.tx.clone());
            self.tails.insert(path, (tag, handle));
        }
    }

    fn abort_all(self) {
        for (_, (_, handle)) in self.tails {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reconcile_follows_the_active_sessions() {
        let projects = tempfile::tempdir().unwrap();
        let log = projects.path().join("proj").join("s1.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "").unwrap();
        let (tx, _rx) = mpsc::channel(8);
        let mut tails = TailSet {
            tx,
            tails: HashMap::new(),
            next_tag: 0,
        };
        let mut now = NowStats::new();

        tails.reconcile(projects.path(), &mut now);
        assert_eq!(tails.tails.len(), 1);
        tails.reconcile(projects.path(), &mut now);
        assert_eq!(tails.next_tag, 1, "a live tail is kept, not respawned");

        std::fs::remove_file(&log).unwrap();
        tails.reconcile(projects.path(), &mut now);
        assert!(tails.tails.is_empty(), "a gone session's tail is dropped");
        tails.abort_all();
    }
}
