//! TUI rendering and event loop.
//!
//! Owns the terminal lifecycle (`ratatui::try_init` / `restore`), drives a
//! `tokio::select!` loop over render-tick / watcher-events / keyboard input,
//! and dispatches per-session events to the matching `SessionEntry`.
//!
//! Two view modes:
//! - `Summary` (default): table of every discovered session, with an
//!   aggregate band and a highlighted selected row.
//! - `Single`: the legacy detail view of one session. Reachable from
//!   Summary via Enter / `s`; pinned when started with `--session`.

mod draw;
mod format;
mod keys;
mod state;

use anyhow::Result;
use crossterm::event::{Event as CtEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::session::{self, DiscoveredSession};
use crate::stats::SessionStats;
use crate::summary::AgentStatus;
use cctk::jsonl::Line as SessionLine;
use cctk::tail;
use draw::draw_frame;
use keys::{KeyAction, classify_key};
use state::{ViewMode, next_selected, prev_selected};

/// Bounded mpsc channel capacity for the shared `(idx, SessionLine)` bus.
/// Scales with the number of watchers; 1024 leaves plenty of headroom
/// for bursts even with dozens of sessions.
const EVENT_CHANNEL_CAPACITY: usize = 1024;
/// Maximum events drained per `recv_many` batch.
const EVENT_DRAIN_BATCH: usize = 128;
/// claude.ai page that shows subscription quota / session reset / weekly limits.
const USAGE_DASHBOARD_URL: &str = "https://claude.ai/settings/usage";

/// Runtime configuration for `ui::run`. Built once from CLI flags + env vars.
pub struct Config {
    pub projects_dir: PathBuf,
    pub explicit_session: Option<PathBuf>,
    pub refresh_ms: u64,
    pub context_window_override: Option<u64>,
    /// When `Some(n)`, the summary list also includes JSONLs touched within
    /// the last `n` days (in addition to live agents). `None` ⇒ live only.
    pub days: Option<u64>,
    /// When `Some(d)`, the session list auto-refreshes every `d` (re-running
    /// `discover_sessions` + reconciling watchers, same as manual `R`).
    /// `None` ⇒ refresh only on `R`.
    pub watch_interval: Option<Duration>,
}

/// Per-session state owned by the UI: the file path, identifiers, status +
/// name pulled from `claude agents --json`, rolling stats, and the watcher
/// task handle (so we can abort on rescan).
struct SessionEntry {
    path: PathBuf,
    short_id: String,
    session_id: String,
    name: Option<String>,
    status: AgentStatus,
    stats: SessionStats,
    handle: JoinHandle<Result<()>>,
}

/// Public entry point. Owns terminal init/restore around `run_inner`. Any
/// error that escapes propagates after the terminal has been restored.
pub async fn run(cfg: Config) -> Result<()> {
    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let res = run_inner(&mut terminal, cfg).await;
    ratatui::restore();
    res
}

/// Core event loop. Drains the shared watcher bus into per-session stats,
/// redraws each tick, and handles key input per the active view mode.
async fn run_inner(terminal: &mut DefaultTerminal, cfg: Config) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::channel::<(usize, SessionLine)>(EVENT_CHANNEL_CAPACITY);

    let (mut entries, pinned, mut mode) = if let Some(path) = cfg.explicit_session.clone() {
        // `--session <path>` mode: single pinned entry, no summary toggle.
        let short_id = session::short_id(&path);
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&short_id)
            .to_string();
        let handle = tail::spawn(path.clone(), 0, event_tx.clone());
        let stats = SessionStats::new(cfg.context_window_override);
        let entry = SessionEntry {
            path,
            short_id,
            session_id,
            name: None,
            status: AgentStatus::Offline,
            stats,
            handle,
        };
        (vec![entry], true, ViewMode::Single)
    } else {
        let discovered = session::discover_sessions(&cfg.projects_dir, cfg.days).await?;
        if discovered.is_empty() {
            anyhow::bail!("no sessions found (start one with `claude` or widen with `--days N`)");
        }
        let entries = spawn_entries(&discovered, &event_tx, cfg.context_window_override);
        (entries, false, ViewMode::Summary)
    };

    let mut selected: usize = 0;
    let mut event_buf: Vec<(usize, SessionLine)> = Vec::with_capacity(EVENT_DRAIN_BATCH);
    let mut tick = tokio::time::interval(Duration::from_millis(cfg.refresh_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Auto-refresh ticker. `None` when `--watch` wasn't passed; the select!
    // arm uses `future::pending` in that case so it never fires.
    let mut watch_tick = cfg.watch_interval.map(|d| {
        let mut i = tokio::time::interval(d);
        // First tick fires immediately; skip it so we don't double-rescan
        // on startup (we just discovered the list moments ago).
        i.reset();
        i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        i
    });
    let mut events = EventStream::new();

    loop {
        terminal.draw(|f| draw_frame(f, &entries, selected, mode))?;

        let watch_fut = async {
            if let Some(t) = watch_tick.as_mut() {
                let _ = t.tick().await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            _ = tick.tick() => {}
            () = watch_fut, if !pinned => {
                // Auto-refresh is best-effort: if `claude agents --json`
                // hiccups (CLI crashed, slow, network), we keep the current
                // view rather than tearing down the TUI. Manual `R` still
                // surfaces the error to the user.
                let _ = rescan(&mut entries, &event_tx, &cfg, &mut selected).await;
            }
            n = event_rx.recv_many(&mut event_buf, EVENT_DRAIN_BATCH) => {
                if n == 0 {
                    return Ok(()); // all watchers closed
                }
                for (idx, ev) in event_buf.drain(..) {
                    if let Some(entry) = entries.get_mut(idx) {
                        entry.stats.ingest(&ev);
                    }
                }
            }
            Some(Ok(ct_ev)) = events.next() => {
                if let CtEvent::Key(k) = ct_ev
                    && k.kind == KeyEventKind::Press
                {
                    match classify_key(k.code, mode, pinned) {
                        KeyAction::Quit => return Ok(()),
                        KeyAction::OpenBrowser => open_in_browser(USAGE_DASHBOARD_URL),
                        KeyAction::ResetCurrent => {
                            if let Some(e) = entries.get_mut(selected) {
                                e.stats.reset();
                            }
                        }
                        KeyAction::SelectPrev | KeyAction::PrevInSingle => {
                            selected = prev_selected(&entries, selected);
                        }
                        KeyAction::SelectNext | KeyAction::NextInSingle => {
                            selected = next_selected(&entries, selected);
                        }
                        KeyAction::OpenSelected => mode = ViewMode::Single,
                        KeyAction::ToggleView => mode = ViewMode::Summary,
                        KeyAction::Rescan => {
                            rescan(
                                &mut entries,
                                &event_tx,
                                &cfg,
                                &mut selected,
                            ).await?;
                        }
                        KeyAction::Ignore => {}
                    }
                }
            }
        }
    }
}

/// Spawn one watcher per discovered session, returning a Vec aligned by
/// index with the `(idx, SessionLine)` tag emitted to the shared channel.
fn spawn_entries(
    discovered: &[DiscoveredSession],
    tx: &mpsc::Sender<(usize, SessionLine)>,
    ctx_override: Option<u64>,
) -> Vec<SessionEntry> {
    discovered
        .iter()
        .enumerate()
        .map(|(idx, d)| SessionEntry::spawn(d, idx, tx, ctx_override))
        .collect()
}

impl SessionEntry {
    /// A fresh entry for `d`, its watcher tagged `idx`.
    fn spawn(
        d: &DiscoveredSession,
        idx: usize,
        tx: &mpsc::Sender<(usize, SessionLine)>,
        ctx_override: Option<u64>,
    ) -> Self {
        let (name, status) = name_and_status(d);
        SessionEntry {
            path: d.path.clone(),
            short_id: d.short_id.clone(),
            session_id: d.session_id.clone(),
            name,
            status,
            stats: SessionStats::new(ctx_override),
            handle: watch(d, idx, tx),
        }
    }

    /// The watcher still runs with the old tag (the previous index): abort
    /// it and watch again tagged `idx`, so its lines reach this entry.
    fn retag(
        &mut self,
        d: &DiscoveredSession,
        idx: usize,
        tx: &mpsc::Sender<(usize, SessionLine)>,
    ) {
        self.handle.abort();
        self.handle = watch(d, idx, tx);
    }
}

/// Tail `d`'s log, tagging each line with `idx`.
fn watch(
    d: &DiscoveredSession,
    idx: usize,
    tx: &mpsc::Sender<(usize, SessionLine)>,
) -> JoinHandle<Result<()>> {
    tail::spawn(d.path.clone(), idx, tx.clone())
}

/// The name and status `claude agents --json` reported for `d`; offline
/// when it isn't listed.
fn name_and_status(d: &DiscoveredSession) -> (Option<String>, AgentStatus) {
    match &d.agent_info {
        Some(info) => (
            info.name.clone(),
            AgentStatus::from_kind(Some(info.status_kind)),
        ),
        None => (None, AgentStatus::Offline),
    }
}

/// Re-discover sessions and reconcile against the running watcher set:
/// keep entries whose `session_id` still appears, abort + drop the rest,
/// spawn watchers for newly-added sessions. Preserves selection by id.
async fn rescan(
    entries: &mut Vec<SessionEntry>,
    tx: &mpsc::Sender<(usize, SessionLine)>,
    cfg: &Config,
    selected: &mut usize,
) -> Result<()> {
    let fresh = session::discover_sessions(&cfg.projects_dir, cfg.days).await?;
    if fresh.is_empty() {
        return Ok(());
    }

    // Capture the currently-selected session id so we can re-locate it.
    let selected_id = entries.get(*selected).map(|e| e.session_id.clone());

    // Build a lookup of existing entries by session_id so we can keep their stats.
    let mut existing: HashMap<String, SessionEntry> = entries
        .drain(..)
        .map(|e| (e.session_id.clone(), e))
        .collect();

    let mut new_entries: Vec<SessionEntry> = Vec::with_capacity(fresh.len());
    for (idx, d) in fresh.iter().enumerate() {
        if let Some(mut e) = existing.remove(&d.session_id) {
            // Refresh status/name from the latest `claude agents --json`.
            (e.name, e.status) = name_and_status(d);
            e.retag(d, idx, tx);
            new_entries.push(e);
        } else {
            new_entries.push(SessionEntry::spawn(d, idx, tx, cfg.context_window_override));
        }
    }

    // Anything left in `existing` was dropped from the discover_sessions output;
    // abort their watchers so we don't leak the tasks.
    for (_id, dropped) in existing {
        dropped.handle.abort();
    }

    *entries = new_entries;
    // Re-locate the previously-selected session by id, if it survived.
    *selected = selected_id
        .and_then(|id| entries.iter().position(|e| e.session_id == id))
        .unwrap_or(0);

    Ok(())
}

fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let _ = std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_status_follow_the_agent_listing() {
        let session = |agent_info| DiscoveredSession {
            path: PathBuf::from("/p/s.jsonl"),
            short_id: "s".into(),
            session_id: "s".into(),
            agent_info,
        };
        let busy = session(Some(session::AgentInfo {
            name: Some("fix-login".into()),
            status_kind: session::AgentStatusKind::Busy,
        }));
        assert_eq!(
            name_and_status(&busy),
            (Some("fix-login".into()), AgentStatus::Busy)
        );
        let idle = session(Some(session::AgentInfo {
            name: None,
            status_kind: session::AgentStatusKind::Idle,
        }));
        assert_eq!(name_and_status(&idle), (None, AgentStatus::Idle));
        assert_eq!(
            name_and_status(&session(None)),
            (None, AgentStatus::Offline)
        );
    }
}
