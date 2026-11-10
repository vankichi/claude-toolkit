//! cctop — unified bottom-style TUI dashboard for Claude Code.
//!
//! One screen that combines the live active-session monitor (Now), the
//! cross-session usage ranking (Top usage), the config map (agents /
//! skills / commands / plugins / MCP), and the ccfocus board of every
//! session's status (Focus). Select a panel with 1/2/3/4 (or j/k) and press
//! Enter to drill into its full view; `/` fuzzy-filters; `q` quits.

mod agents;
mod app;
mod now;
mod overview;
mod run;
mod store;

use anyhow::{Context, Result};
use clap::Parser;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "cctop",
    version,
    about = "Unified TUI dashboard for Claude Code: live usage, stats, and config map"
)]
struct Cli {
    #[command(flatten)]
    projects: cctk::cli::ProjectsDir,

    #[command(flatten)]
    claude: cctk::cli::ClaudeDir,

    #[command(flatten)]
    project: cctk::cli::ProjectDir,

    #[command(flatten)]
    refresh: cctk::cli::Refresh,

    /// How often (seconds) to re-scan the corpus and re-discover the config
    /// map. The active-session "Now" panel updates live regardless.
    #[arg(long, default_value_t = 5)]
    rescan_secs: u64,
}

// `projects_dir` (~/.claude/projects, session logs) and `project_dir` (the
// cwd scanned for .claude/.mcp.json) are established, distinct domain terms
// carried over from ccwatch/ccmap; the similarity is intentional.
#[expect(clippy::similar_names)]
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let projects_dir = cli.projects.resolve()?;
    let claude_dir = cli.claude.resolve()?;
    let project_dir = match cli.project.project_dir {
        Some(p) => p,
        None => std::env::current_dir().context("could not determine current directory")?,
    };

    // Where the hook writes (`$CCFOCUS_STATE_DIR`, else ~/.claude/focus);
    // `--claude-dir` does not move it.
    let focus_dir = cctk::focus::default_state_dir().unwrap_or_else(|| claude_dir.join("focus"));

    run::run(run::RunConfig {
        projects_dir,
        claude_dir,
        project_dir,
        focus_dir,
        refresh: cli.refresh.interval(),
        rescan_every: Duration::from_secs(cli.rescan_secs.max(1)),
    })
    .await
}
