//! ccfocus — hook-driven status board of every Claude Code session.
//!
//! `ccfocus hook <event>` is meant to be registered as a Claude Code command
//! hook. Whatever goes wrong, it prints at most one line to stderr and exits 0
//! so Claude's work is never blocked (exit 2 would block a prompt or a stop),
//! and it never writes to stdout (`SessionStart` / `UserPromptSubmit` stdout
//! is injected into Claude's context).

use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

/// Hook input larger than this is rejected (Stop carries the last assistant
/// message, so inputs can be large, but never this large).
const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;

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
                eprintln!("ccfocus: {}", msg.lines().next().unwrap_or("bad arguments"));
            }
            return ExitCode::SUCCESS;
        }
    };
    let Command::Hook { event, state_dir } = cli.command;
    if let Err(e) = hook(&event, state_dir) {
        eprintln!("ccfocus: {event}: {e:#}");
    }
    ExitCode::SUCCESS
}

fn hook(event: &str, state_dir: Option<PathBuf>) -> Result<()> {
    let state_dir = state_dir
        .or_else(cctk::focus::default_state_dir)
        .context("could not determine the state directory")?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT_BYTES)
        .read_to_end(&mut input)
        .context("read stdin")?;
    let now = chrono::Local::now().fixed_offset();
    ccfocus::handle(event, &input, &state_dir, now, &ccfocus::probe::SystemProbe)?;
    Ok(())
}
