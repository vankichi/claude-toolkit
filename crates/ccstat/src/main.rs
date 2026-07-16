//! ccstat — terminal dashboard of Claude Code usage across sessions.

use anyhow::{Context, Result};
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
    let projects_dir = if let Some(p) = cli.projects_dir {
        p
    } else {
        let home = std::env::home_dir().context("could not determine home directory")?;
        home.join(".claude").join("projects")
    };
    ui::run(ScanConfig { projects_dir }, Utc::now().date_naive())
}
