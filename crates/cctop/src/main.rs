//! cctop — unified bottom-style TUI dashboard for Claude Code.
//!
//! One screen that combines the live active-session monitor (Now), the
//! cross-session usage ranking (Top usage), and the config map (agents /
//! skills / commands / plugins / MCP). Select a panel with 1/2/3 (or j/k) and
//! press Enter to drill into its full view; `/` fuzzy-filters; `q` quits.

mod app;
mod now;
mod overview;
mod run;
mod store;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "cctop",
    version,
    about = "Unified TUI dashboard for Claude Code: live usage, stats, and config map"
)]
struct Cli {
    /// Override the Claude projects directory (default: ~/.claude/projects)
    #[arg(long)]
    projects_dir: Option<PathBuf>,

    /// Override the Claude config directory (default: ~/.claude)
    #[arg(long)]
    claude_dir: Option<PathBuf>,

    /// Override the project directory scanned for `.claude` and `.mcp.json`
    /// (default: the current working directory)
    #[arg(long)]
    project_dir: Option<PathBuf>,

    /// UI redraw interval in milliseconds
    #[arg(long, default_value_t = 250)]
    refresh_ms: u64,

    /// How often (seconds) to re-scan the corpus and re-discover the config
    /// map. The active-session "Now" panel updates live regardless.
    #[arg(long, default_value_t = 5)]
    rescan_secs: u64,
}

// `projects_dir` (~/.claude/projects, session logs) and `project_dir` (the
// cwd scanned for .claude/.mcp.json) are established, distinct domain terms
// carried over from ccwatch/ccmap; the similarity is intentional.
#[allow(clippy::similar_names)]
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = std::env::home_dir();

    let projects_dir = match cli.projects_dir {
        Some(p) => p,
        None => home
            .clone()
            .context("could not determine home directory")?
            .join(".claude")
            .join("projects"),
    };
    let claude_dir = match cli.claude_dir {
        Some(p) => p,
        None => home
            .context("could not determine home directory")?
            .join(".claude"),
    };
    let project_dir = match cli.project_dir {
        Some(p) => p,
        None => std::env::current_dir().context("could not determine current directory")?,
    };

    run::run(run::RunConfig {
        projects_dir,
        claude_dir,
        project_dir,
        refresh: Duration::from_millis(cli.refresh_ms.max(16)),
        rescan_every: Duration::from_secs(cli.rescan_secs.max(1)),
    })
    .await
}
