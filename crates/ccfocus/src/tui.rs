//! `ccfocus` / `ccfocus tui`: the focus board full screen (`--tiny`: the
//! card view for a narrow side pane). A plain synchronous loop: poll the
//! keyboard briefly, tick, draw. The worktree scan, gh and a jump's tmux
//! commands run on background `Job` threads; what stays on this thread
//! is bounded by timeouts (the `d` / `p` keys' tmux / merge-base call, the
//! periodic `kill -0` checks, and on quit waiting for a jump and killing
//! the git / gh still running).

use crate::app::Focus;
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;
use std::time::Duration;

/// Keyboard poll per loop; also the redraw cadence while idle.
const POLL: Duration = Duration::from_millis(250);

pub fn run(state_dir: PathBuf, tiny: bool) -> Result<()> {
    let draw = if tiny {
        crate::ui::draw_tiny
    } else {
        crate::ui::draw
    };
    let mut focus = Focus::open(state_dir);
    focus.tick(true);
    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|f| draw(f, f.area(), focus.view()))?;
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
    // A jump key just before `q`: let the switch finish while the board
    // (showing "switching…") is still on screen. Then stop git / gh.
    focus.close();
    ratatui::restore();
    result
}
