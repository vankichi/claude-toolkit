//! ccfocus — hook-driven status board of every Claude Code session.
//!
//! `ccfocus hook <event>` is meant to be registered as a Claude Code command
//! hook. Whatever goes wrong, it prints at most one line to stderr and exits 0
//! so Claude's work is never blocked (exit 2 would block a prompt or a stop),
//! and the hook path never writes to stdout (`SessionStart` /
//! `UserPromptSubmit` stdout is injected into Claude's context); only
//! `--help` / `--version` print there.

use anyhow::{Context, Result, bail};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// Hook input larger than this is refused (Stop carries the last assistant
/// message, so inputs can be large, but never this large).
const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;

/// Total time all external probes (git, tmux, hostname) may take per event.
/// They normally finish in tens of milliseconds; the cap bounds how long a
/// hung command can delay a prompt.
const PROBE_BUDGET: Duration = Duration::from_secs(1);

#[derive(Parser, Debug)]
#[command(
    name = "ccfocus",
    version,
    about = "Status board of every Claude Code session (waiting / working / done)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Record a Claude Code hook event read as JSON from stdin. Register it for
    /// `SessionStart`, `UserPromptSubmit`, `Notification`, `Stop`, and
    /// `SessionEnd`.
    Hook {
        /// Hook event name, e.g. `SessionStart`
        event: String,

        /// State directory (default: `$CCFOCUS_STATE_DIR`, else ~/.claude/focus)
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                let _ = e.print();
            } else {
                let msg = e.to_string();
                warn(msg.lines().next().unwrap_or("bad arguments"));
            }
            return ExitCode::SUCCESS;
        }
    };
    let Command::Hook { event, state_dir } = cli.command;
    if let Err(e) = hook(&event, state_dir) {
        warn(&format!("{event}: {e:#}"));
    }
    ExitCode::SUCCESS
}

/// One stderr line. Unlike `eprintln!`, a failed write is ignored rather than
/// panicking (the release profile aborts on panic, which would not exit 0).
fn warn(msg: &str) {
    let _ = writeln!(std::io::stderr(), "ccfocus: {msg}");
}

fn hook(event: &str, state_dir: Option<PathBuf>) -> Result<()> {
    let state_dir = state_dir
        .or_else(cctk::focus::default_state_dir)
        .context("could not determine the state directory")?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut input)
        .context("read stdin")?;
    if input.len() as u64 > MAX_INPUT_BYTES {
        bail!("hook input exceeds {MAX_INPUT_BYTES} bytes");
    }
    let now = chrono::Local::now().fixed_offset();
    let probe = ccfocus::probe::SystemProbe::new(PROBE_BUDGET);
    ccfocus::handle(event, &input, &state_dir, now, &probe)?;
    Ok(())
}
