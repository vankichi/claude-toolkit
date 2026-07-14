//! ccmap — terminal explorer for Claude Code agents, skills, commands,
//! plugins, and MCP servers.

use anyhow::{Context, Result};
use ccmap::discover;
use ccmap::ui::{self, UiConfig};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "ccmap",
    version,
    about = "Terminal explorer for Claude Code agents, skills, commands, plugins, and MCP servers"
)]
struct Cli {
    /// Override the Claude config directory (default: ~/.claude)
    #[arg(long)]
    claude_dir: Option<PathBuf>,

    /// Override the project directory to scan for `.claude` and `.mcp.json`
    /// (default: the current working directory)
    #[arg(long)]
    project_dir: Option<PathBuf>,
}

/// Binary entry point. Parses CLI args, resolves the discovery roots, then
/// hands off to `ui::run`, which owns the terminal lifecycle and the
/// (synchronous, non-tokio) event loop.
fn main() -> Result<()> {
    let cli = Cli::parse();

    let claude_dir = if let Some(dir) = cli.claude_dir {
        dir
    } else {
        let home = std::env::home_dir().context("could not determine home directory")?;
        home.join(".claude")
    };

    let project_dir = match cli.project_dir {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };

    ui::run(UiConfig {
        ctx: discover::Context {
            claude_dir,
            project_dir,
        },
    })
}
