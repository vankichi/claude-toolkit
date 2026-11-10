//! Command-line flags several tools share. Each tool flattens the ones it
//! takes into its clap parser (`#[command(flatten)]`), so a flag has one
//! name, help text and default everywhere.

use std::path::PathBuf;
use std::time::Duration;

/// `--projects-dir`: where the session logs live.
#[derive(clap::Args, Debug, Clone)]
pub struct ProjectsDir {
    /// Override the Claude projects directory (default: ~/.claude/projects)
    #[arg(long)]
    pub projects_dir: Option<PathBuf>,
}

impl ProjectsDir {
    /// The flag's value, else `~/.claude/projects`.
    ///
    /// # Errors
    /// When the flag is absent and the home directory can't be determined.
    pub fn resolve(self) -> std::io::Result<PathBuf> {
        self.projects_dir
            .map_or_else(crate::paths::default_projects_dir, Ok)
    }
}

/// `--claude-dir`: the Claude config directory.
#[derive(clap::Args, Debug, Clone)]
pub struct ClaudeDir {
    /// Override the Claude config directory (default: ~/.claude)
    #[arg(long)]
    pub claude_dir: Option<PathBuf>,
}

impl ClaudeDir {
    /// The flag's value, else `~/.claude`.
    ///
    /// # Errors
    /// When the flag is absent and the home directory can't be determined.
    pub fn resolve(self) -> std::io::Result<PathBuf> {
        self.claude_dir
            .map_or_else(crate::paths::default_claude_dir, Ok)
    }
}

/// `--project-dir`: the project whose `.claude` and `.mcp.json` are read.
#[derive(clap::Args, Debug, Clone)]
pub struct ProjectDir {
    /// Override the project directory scanned for `.claude` and `.mcp.json`
    /// (default: the current working directory)
    #[arg(long)]
    pub project_dir: Option<PathBuf>,
}

impl ProjectDir {
    /// The flag's value, else the current working directory.
    ///
    /// # Errors
    /// When the flag is absent and the current directory can't be read.
    pub fn resolve(self) -> anyhow::Result<PathBuf> {
        use anyhow::Context;
        match self.project_dir {
            Some(dir) => Ok(dir),
            None => std::env::current_dir().context("could not determine current directory"),
        }
    }
}

/// The lowest `--refresh-ms` honoured: smaller values, 0 included, are
/// raised to it (a zero period would panic the redraw timer).
pub const MIN_REFRESH_MS: u64 = 16;

/// `--refresh-ms`: how often the TUI redraws. Read it through
/// [`Refresh::millis`] / [`Refresh::interval`], which apply the floor.
#[derive(clap::Args, Debug, Clone)]
pub struct Refresh {
    /// UI redraw interval in milliseconds (minimum 16)
    #[arg(long, default_value_t = 250)]
    refresh_ms: u64,
}

impl Refresh {
    /// The interval in milliseconds, raised to [`MIN_REFRESH_MS`].
    #[must_use]
    pub fn millis(&self) -> u64 {
        self.refresh_ms.max(MIN_REFRESH_MS)
    }

    /// [`Refresh::millis`] as a `Duration`.
    #[must_use]
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        projects: ProjectsDir,
        #[command(flatten)]
        refresh: Refresh,
    }

    #[test]
    fn refresh_is_floored_and_defaults_to_250() {
        let cli = Cli::parse_from(["t"]);
        assert_eq!(cli.refresh.millis(), 250);
        assert_eq!(
            Cli::parse_from(["t", "--refresh-ms", "0"]).refresh.millis(),
            MIN_REFRESH_MS
        );
        assert_eq!(
            Cli::parse_from(["t", "--refresh-ms", "100"])
                .refresh
                .interval(),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn a_given_projects_dir_wins() {
        let cli = Cli::parse_from(["t", "--projects-dir", "/logs"]);
        assert_eq!(cli.projects.resolve().unwrap(), PathBuf::from("/logs"));
    }

    #[test]
    fn the_help_states_the_floor() {
        use clap::CommandFactory;
        let help = Cli::command().render_long_help().to_string();
        assert!(
            help.contains(&format!("(minimum {MIN_REFRESH_MS})")),
            "{help}"
        );
    }
}
