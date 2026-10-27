//! `ccfocus` / `ccfocus tui`: the focus board full screen. A plain
//! synchronous loop: poll the keyboard briefly, tick, draw. The worktree scan
//! and gh run on [`crate::repos::Job`] threads; what stays on this thread is
//! bounded by timeouts (a key action's tmux / merge-base call, the periodic
//! `kill -0` checks).

use crate::app::Focus;
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;
use std::time::Duration;

/// Keyboard poll per loop; also the redraw cadence while idle.
const POLL: Duration = Duration::from_millis(250);

pub fn run(state_dir: PathBuf) -> Result<()> {
    let mut focus = Focus::open(state_dir);
    focus.tick(true);
    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|f| crate::ui::draw(f, f.area(), focus.view()))?;
            if event::poll(POLL)?
                && let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
                && focus.on_key(k.code)
            {
                return Ok(());
            }
            focus.tick(false);
        }
    })();
    ratatui::restore();
    result
}
