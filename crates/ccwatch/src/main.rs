//! ccwatch — real-time terminal monitor for Claude Code session usage.

mod jsonl;
mod pricing;
mod session;
mod stats;
mod summary;
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

    /// Override the context window size (e.g. 200000 to opt down from the
    /// 1M default). Falls back to the `CCWATCH_CONTEXT_WINDOW` env var.
    /// Without either, ccwatch assumes 1M for every model family because
    /// Claude Code now defaults to the 1M-context tier.
    #[arg(long, env = "CCWATCH_CONTEXT_WINDOW")]
    context_window: Option<u64>,

    /// Also include historic sessions whose JSONL was modified within the
    /// last N days. By default the summary lists only live agents reported
    /// by `claude agents --json`.
    #[arg(long, conflicts_with = "session")]
    days: Option<u64>,

    /// Auto-refresh the session list every N seconds (re-query
    /// `claude agents --json` and reconcile watchers). `--watch` alone
    /// uses the 5-second default. Without this flag, the list only
    /// refreshes on manual `R`.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "5",
        value_name = "SECONDS",
        conflicts_with = "session"
    )]
    watch: Option<u64>,
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

    // Zero is treated the same as absent — guards `tokio::time::interval(0)`
    // which panics, and gives users a way to disable via `--watch 0`.
    let watch_interval = cli
        .watch
        .filter(|&s| s > 0)
        .map(std::time::Duration::from_secs);

    ui::run(ui::Config {
        projects_dir,
        explicit_session: cli.session,
        refresh_ms: cli.refresh_ms,
        context_window_override: cli.context_window,
        days: cli.days,
        watch_interval,
    })
    .await
}
