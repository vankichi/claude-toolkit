//! ccstat — terminal dashboard of Claude Code usage across sessions.

use anyhow::{Context, Result};
use ccmap::discover;
use ccstat::scan::ScanConfig;
use ccstat::ui;
use chrono::Utc;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "ccstat",
    version,
    about = "Terminal dashboard of Claude Code model/agent/skill/command/MCP usage across sessions"
)]
struct Cli {
    /// Override Claude projects directory (default: ~/.claude/projects)
    #[arg(long)]
    projects_dir: Option<PathBuf>,

    /// Live mode: spin a loading indicator on items running now and
    /// auto-refresh. Optionally set the full-rescan interval in SECONDS
    /// (default 300 = 5 min). `--watch 0` disables live mode.
    #[arg(long, num_args = 0..=1, default_missing_value = "300", value_name = "SECONDS")]
    watch: Option<u64>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = std::env::home_dir().context("could not determine home directory")?;
    let projects_dir = cli
        .projects_dir
        .unwrap_or_else(|| home.join(".claude").join("projects"));
    let cfg = ScanConfig { projects_dir };
    let ctx = discover::Context {
        claude_dir: home.join(".claude"),
        project_dir: std::env::current_dir().context("could not determine current directory")?,
    };
    // Zero disables live mode (and guards against a 0-length rescan interval).
    let watch = cli
        .watch
        .filter(|&s| s > 0)
        .map(std::time::Duration::from_secs);
    ui::run(&cfg, &ctx, Utc::now().date_naive(), watch)
}
