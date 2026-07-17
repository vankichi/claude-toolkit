//! Async runtime: owns the terminal, tails the active session, periodically
//! rescans the stats/map subsystems, and multiplexes tail events / render
//! ticks / keyboard input in a single `tokio::select!` loop.
//!
//! Rendering and key dispatch branch on [`App::mode`]: the overview draws all
//! three panels, while a drilled-in panel renders and drives the corresponding
//! full view (`ccstat`/`ccmap` re-entrant API, or the self-rendered Now
//! detail).

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
use std::path::PathBuf;
use std::time::Duration;
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
    let mut active = pick_active(&cfg.projects_dir);
    let mut tail_handle = active.clone().map(|p| cctk::tail::spawn(p, 0, tx.clone()));

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
        &mut active,
        &mut tail_handle,
    )
    .await;
    ratatui::restore();
    if let Some(h) = tail_handle {
        h.abort();
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
    active: &mut Option<PathBuf>,
    tail_handle: &mut Option<JoinHandle<Result<()>>>,
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
                if let Some((_, line)) = maybe {
                    dash.now.ingest(&line);
                }
            }
            _ = render_tick.tick() => {}
            _ = rescan_tick.tick() => {
                let today = Utc::now().date_naive();
                dash.rescan(scan_cfg, ctx, today);
                reconcile_active(&cfg.projects_dir, dash, tx, active, tail_handle);
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

/// The most-recently-modified session file under `projects_dir`, if any.
fn pick_active(projects_dir: &std::path::Path) -> Option<PathBuf> {
    cctk::paths::session_files(projects_dir)
        .into_iter()
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((p, mtime))
        })
        .max_by_key(|(_, m)| *m)
        .map(|(p, _)| p)
}

/// After a rescan, if a newer active session appeared, abort the old tail,
/// reset the Now aggregate, and tail the new file.
fn reconcile_active(
    projects_dir: &std::path::Path,
    dash: &mut Dashboard,
    tx: &mpsc::Sender<(usize, Line)>,
    active: &mut Option<PathBuf>,
    tail_handle: &mut Option<JoinHandle<Result<()>>>,
) {
    let newest = pick_active(projects_dir);
    if newest == *active {
        return;
    }
    if let Some(h) = tail_handle.take() {
        h.abort();
    }
    dash.now = crate::now::NowStats::new();
    if let Some(p) = &newest {
        *tail_handle = Some(cctk::tail::spawn(p.clone(), 0, tx.clone()));
    }
    *active = newest;
}
