//! ccmap — terminal explorer for Claude Code agents, skills, commands,
//! plugins, and MCP servers.

use anyhow::{Context, Result};
use ccmap::discover;
use ccmap::ui::{self, UiConfig};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "ccmap",
    version,
    about = "Terminal explorer for Claude Code agents, skills, commands, plugins, and MCP servers"
)]
struct Cli {
    #[command(flatten)]
    claude: cctk::cli::ClaudeDir,

    #[command(flatten)]
    project: cctk::cli::ProjectDir,
}

/// Binary entry point. Parses CLI args, resolves the discovery roots, then
/// hands off to `ui::run`, which owns the terminal lifecycle and the
/// (synchronous, non-tokio) event loop.
fn main() -> Result<()> {
    let cli = Cli::parse();

    let claude_dir = cli.claude.resolve()?;

    let project_dir = match cli.project.project_dir {
        Some(dir) => dir,
        None => std::env::current_dir().context("could not determine current directory")?,
    };

    ui::run(UiConfig {
        ctx: discover::Context {
            claude_dir,
            project_dir,
        },
    })
}
