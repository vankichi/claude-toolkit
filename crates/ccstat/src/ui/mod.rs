//! ccstat TUI: pure `AppState` (fully testable without a terminal), key
//! classification, detail formatting, rendering, and the two event loops —
//! the blocking snapshot loop and the `--watch` live loop that animates the
//! running-item spinner and auto-refreshes.

mod draw;
mod format;
mod keys;
mod state;
#[cfg(test)]
mod test_support;

pub use draw::{draw, trends_series};
pub use format::{color_for_name, detail_lines, format_recency};
pub use keys::{KeyAction, classify_key, handle_key};
pub use state::AppState;

use crate::provenance::ProvenanceMap;
use crate::scan::{self, ScanConfig};
use chrono::{Duration, NaiveDate};
use crossterm::event::{Event as CtEvent, KeyEventKind};
use ratatui::DefaultTerminal;

/// Spinner animation frame interval (and the max time an idle watch loop
/// blocks on input before redrawing).
const SPINNER_TICK: std::time::Duration = std::time::Duration::from_millis(100);
/// How often watch mode recomputes the "running now" set (cheap tail reads).
const ACTIVE_POLL: std::time::Duration = std::time::Duration::from_secs(2);
/// A session counts as live if its log was modified within this many seconds.
const ACTIVE_WINDOW_SECS: i64 = 90;
/// Bytes read from the end of each live session log for activity detection.
const TAIL_BYTES: u64 = 16 * 1024;

/// Runs the ccstat TUI to completion: scans logs, discovers extension
/// provenance, enters the alternate screen + raw mode, and drives the event
/// loop until the user quits. `watch` selects the loop: `None` is the classic
/// blocking snapshot loop; `Some(interval)` is the live loop that animates the
/// spinner, polls activity, and re-aggregates every `interval`. The terminal
/// is always restored, on success and error alike.
pub fn run(
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
    today: NaiveDate,
    watch: Option<std::time::Duration>,
) -> anyhow::Result<()> {
    let db = scan::scan(cfg, today);
    let provenance = ProvenanceMap::build(&ccmap::discover::discover_extensions(ctx));
    let mut state = AppState::new(db, provenance, today);
    state.watch = watch.is_some();
    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let result = match watch {
        Some(interval) => run_watch_inner(&mut terminal, cfg, ctx, &mut state, interval),
        None => run_inner(&mut terminal, cfg, ctx, &mut state),
    };
    ratatui::restore();
    result
}

/// Live event loop for `--watch`. Uses `event::poll` so the loop wakes on the
/// spinner timer even without input: each timeout advances the animation,
/// `ACTIVE_POLL` recomputes the running set from live-session tails, and
/// `rescan_interval` re-aggregates the whole corpus (like pressing `R`).
fn run_watch_inner(
    terminal: &mut DefaultTerminal,
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
    state: &mut AppState,
    rescan_interval: std::time::Duration,
) -> anyhow::Result<()> {
    // Seed the running set so the very first frame reflects live activity.
    refresh_active(state, cfg);
    let mut last_active = std::time::Instant::now();
    let mut last_rescan = std::time::Instant::now();

    loop {
        terminal.draw(|f| draw(f, state))?;

        if crossterm::event::poll(SPINNER_TICK)? {
            if let CtEvent::Key(key) = crossterm::event::read()?
                && key.kind == KeyEventKind::Press
                && handle_key(key.code, state, cfg, ctx)
            {
                return Ok(());
            }
        } else {
            // Poll timed out with no input: advance the spinner animation.
            state.tick_spinner();
        }

        let now = std::time::Instant::now();
        if now.duration_since(last_active) >= ACTIVE_POLL {
            refresh_active(state, cfg);
            last_active = now;
        }
        if now.duration_since(last_rescan) >= rescan_interval {
            rescan(state, cfg, ctx);
            last_rescan = now;
        }
    }
}

/// Recompute what is running now from the live sessions' tails.
fn refresh_active(state: &mut AppState, cfg: &ScanConfig) {
    state.set_active(scan::compute_active(
        cfg,
        chrono::Utc::now(),
        Duration::seconds(ACTIVE_WINDOW_SECS),
        TAIL_BYTES,
    ));
}

/// Re-aggregate the whole corpus and re-discover provenance (the `R` key,
/// and every rescan interval in `--watch`).
fn rescan(state: &mut AppState, cfg: &ScanConfig, ctx: &ccmap::discover::Context) {
    let today = chrono::Utc::now().date_naive();
    let provenance = ProvenanceMap::build(&ccmap::discover::discover_extensions(ctx));
    state.reload_at(today, scan::scan(cfg, today), provenance);
}

fn run_inner(
    terminal: &mut DefaultTerminal,
    cfg: &ScanConfig,
    ctx: &ccmap::discover::Context,
    state: &mut AppState,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| draw(f, state))?;
        let CtEvent::Key(key) = crossterm::event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if handle_key(key.code, state, cfg, ctx) {
            return Ok(());
        }
    }
}
