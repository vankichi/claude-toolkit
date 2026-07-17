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
    ui::run(&cfg, &ctx, Utc::now().date_naive())
}
