//! ccwatch — real-time terminal monitor for Claude Code session usage.

mod jsonl;
mod pricing;
mod session;
mod stats;
mod ui;
mod watcher;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "ccwatch",
    version,
    about = "Real-time terminal monitor for Claude Code session usage"
)]
struct Cli {
    /// Override Claude projects directory (default: ~/.claude/projects)
    #[arg(long)]
    projects_dir: Option<PathBuf>,

    /// Watch a specific session JSONL file instead of auto-detecting
    #[arg(long)]
    session: Option<PathBuf>,

    /// Refresh interval in milliseconds for UI redraw
    #[arg(long, default_value_t = 250)]
    refresh_ms: u64,

    /// Override context window size (e.g. 1000000 for 1M-context variants).
    /// Falls back to the `CCWATCH_CONTEXT_WINDOW` env var. If neither is set,
    /// ccwatch detects from the model name and auto-promotes to 1M when
    /// observed context exceeds 200k.
    #[arg(long, env = "CCWATCH_CONTEXT_WINDOW")]
    context_window: Option<u64>,
}

/// Binary entry point. Parses CLI args, resolves the projects directory,
/// then hands off to `ui::run` which owns the terminal lifecycle and the
/// event loop.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let projects_dir = if let Some(p) = cli.projects_dir {
        p
    } else {
        let home = std::env::home_dir().context("could not determine home directory")?;
        home.join(".claude").join("projects")
    };

    ui::run(ui::Config {
        projects_dir,
        explicit_session: cli.session,
        refresh_ms: cli.refresh_ms,
        context_window_override: cli.context_window,
    })
    .await
}
