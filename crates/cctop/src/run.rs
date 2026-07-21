//! Async runtime: owns the terminal, tails every currently-active session,
//! periodically rescans the stats/map subsystems, and multiplexes tail events
//! / render ticks / keyboard input in a single `tokio::select!` loop.
//!
//! Rendering and key dispatch branch on [`App::mode`]: the overview draws all
//! three panels, while a drilled-in panel renders and drives the corresponding
//! full view (`ccstat`/`ccmap` re-entrant API, or the self-rendered Now
//! detail). The Now panel rolls up all active sessions (see [`crate::now`]).

use crate::app::{Action, App, Mode, Panel};
use crate::overview;
use crate::store::Dashboard;
use anyhow::Result;
use ccmap::discover::Context;
use ccstat::scan::ScanConfig;
use cctk::jsonl::Line;
use chrono::Utc;
use crossterm::event::{Event as CtEvent, EventStream, KeyCode, KeyEventKind};
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
    pub refresh: Duration,
    pub rescan_every: Duration,
}

const TAIL_CHANNEL_CAPACITY: usize = 1024;
/// A session whose log was modified within this window counts as "active" and
/// is tailed into the Now rollup. Matches the rate chart's 15-minute window so
/// tailed sessions always have data to plot.
const ACTIVE_WINDOW: Duration = Duration::from_secs(900);

/// A live tail: its channel tag and the abortable task handle.
type Tail = (usize, JoinHandle<Result<()>>);

/// Entry point: build the store, init the terminal, run the loop, restore.
pub async fn run(cfg: RunConfig) -> Result<()> {
    let scan_cfg = ScanConfig {
        projects_dir: cfg.projects_dir.clone(),
    };
    let ctx = Context {
        claude_dir: cfg.claude_dir.clone(),
        project_dir: cfg.project_dir.clone(),
    };
    let today = Utc::now().date_naive();

    let mut dash = Dashboard::load(&scan_cfg, &ctx, today);
    let mut app = App::new();

    let (tx, rx) = mpsc::channel::<(usize, Line)>(TAIL_CHANNEL_CAPACITY);
    let mut tails: HashMap<PathBuf, Tail> = HashMap::new();
    let mut next_tag: usize = 0;
    reconcile_tails(&cfg.projects_dir, &mut dash, &tx, &mut tails, &mut next_tag);

    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let result = run_loop(
        &mut terminal,
        &scan_cfg,
        &ctx,
        &cfg,
        &mut dash,
        &mut app,
        rx,
        &tx,
        &mut tails,
        &mut next_tag,
    )
    .await;
    ratatui::restore();
    for (_, (_, handle)) in tails {
        handle.abort();
    }
    result
}

#[allow(clippy::too_many_arguments)] // wiring the single event loop; splitting would just shuffle state
async fn run_loop(
    terminal: &mut DefaultTerminal,
    scan_cfg: &ScanConfig,
    ctx: &Context,
    cfg: &RunConfig,
    dash: &mut Dashboard,
    app: &mut App,
    mut rx: mpsc::Receiver<(usize, Line)>,
    tx: &mpsc::Sender<(usize, Line)>,
    tails: &mut HashMap<PathBuf, Tail>,
    next_tag: &mut usize,
) -> Result<()> {
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
            _ = render_tick.tick() => {}
            _ = rescan_tick.tick() => {
                let today = Utc::now().date_naive();
                dash.rescan(scan_cfg, ctx, today);
                reconcile_tails(&cfg.projects_dir, dash, tx, tails, next_tag);
            }
            maybe_ev = events.next() => {
                let Some(Ok(ev)) = maybe_ev else { continue };
                if let CtEvent::Key(k) = ev
                    && k.kind == KeyEventKind::Press
                    && handle_key(k.code, terminal, app, dash, scan_cfg, ctx)?
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

/// Reconcile the set of live tails against the currently-active sessions: spawn
/// a tail for each newly-active session and abort (and forget) those that went
/// idle, so the Now rollup tracks exactly the active set.
fn reconcile_tails(
    projects_dir: &Path,
    dash: &mut Dashboard,
    tx: &mpsc::Sender<(usize, Line)>,
    tails: &mut HashMap<PathBuf, Tail>,
    next_tag: &mut usize,
) {
    let desired: Vec<PathBuf> = active_sessions(projects_dir);

    // Drop tails whose session is no longer active.
    let gone: Vec<PathBuf> = tails
        .keys()
        .filter(|p| !desired.contains(p))
        .cloned()
        .collect();
    for path in gone {
        if let Some((tag, handle)) = tails.remove(&path) {
            handle.abort();
            dash.now.drop_session(tag);
        }
    }

    // Spawn tails for newly-active sessions.
    for path in desired {
        if tails.contains_key(&path) {
            continue;
        }
        let tag = *next_tag;
        *next_tag += 1;
        let handle = cctk::tail::spawn(path.clone(), tag, tx.clone());
        tails.insert(path, (tag, handle));
    }
}
