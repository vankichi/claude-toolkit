//! The dashboard's key actions as tmux commands. Planning is pure (an
//! argument list or the reason it can't be done); [`run`] executes one.
//! Arguments go to tmux directly, never through a shell.

use crate::board::{Row, Shown};
use crate::config::Config;
use cctk::focus::HostKind;
use std::process::Command;
use std::time::Duration;

/// Longest window name `p` sets.
const MAX_WINDOW_NAME: usize = 40;

const TMUX_TIMEOUT: Duration = Duration::from_secs(2);

/// The tmux client this dashboard runs in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Client {
    /// `$TMUX` is set.
    pub inside: bool,
    /// `$TMUX_PANE`, validated.
    pub pane: Option<String>,
}

impl Client {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            inside: std::env::var_os("TMUX").is_some_and(|v| !v.is_empty()),
            pane: std::env::var("TMUX_PANE")
                .ok()
                .filter(|p| crate::probe::is_pane_id(p)),
        }
    }
}

pub type Plan = Result<Vec<String>, String>;

fn need_tmux(client: &Client) -> Result<(), String> {
    if client.inside {
        Ok(())
    } else {
        Err("tmux の中で起動すると使えます".into())
    }
}

/// Switch to the session's pane. A container session (whose pane lives in
/// the host tmux, in the window that runs the container) goes to
/// `config.container_target`.
pub fn jump(row: &Row, client: &Client, config: &Config) -> Plan {
    need_tmux(client)?;
    if row.shown == Shown::Stopped {
        return Err("止まっている session です".into());
    }
    if !row.local {
        return if row.host == HostKind::Container {
            Ok(args(&["switch-client", "-t", &config.container_target]))
        } else {
            Err("相手側の host の session です".into())
        };
    }
    match &row.tmux_pane {
        Some(pane) => Ok(args(&["switch-client", "-t", pane])),
        None => Err("tmux の外の session です".into()),
    }
}

/// Open `hunk diff <merge-base> --watch` beside the dashboard, in the
/// session's worktree. `base` is the merge-base the caller computed.
pub fn diff(row: &Row, client: &Client, base: Option<&str>) -> Plan {
    need_tmux(client)?;
    if !row.local {
        return Err("相手側の host の worktree は開けません".into());
    }
    let Some(worktree) = row.worktree_path.as_ref().and_then(|p| p.to_str()) else {
        return Err("git の外の session です".into());
    };
    let Some(base) = base else {
        return Err("origin/HEAD との merge-base がありません".into());
    };
    let mut a = args(&["split-window", "-h"]);
    if let Some(pane) = &client.pane {
        a.extend(args(&["-t", pane]));
    }
    a.extend(args(&["-c", worktree, "hunk", "diff", base, "--watch"]));
    Ok(a)
}

/// Name the session's tmux window after its project.
pub fn rename(row: &Row, client: &Client) -> Plan {
    need_tmux(client)?;
    if !row.local {
        return Err("相手側の host の session です".into());
    }
    let Some(pane) = &row.tmux_pane else {
        return Err("tmux の外の session です".into());
    };
    if row.project.is_empty() {
        return Err("project 名がありません (claude -n で付けられます)".into());
    }
    let name = crate::text::fit(&row.project, MAX_WINDOW_NAME);
    // `--`: a project like `-a todo` must not be read as flags.
    Ok(args(&["rename-window", "-t", pane, "--", &name]))
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_string()).collect()
}

/// Run `tmux <args>`. `Err` carries tmux's first stderr line.
pub fn run(args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("tmux");
    cmd.args(args);
    match crate::cmd::run(cmd, TMUX_TIMEOUT) {
        Some(o) if o.success => Ok(()),
        Some(o) => Err(format!("tmux: {}", crate::cmd::first_line(&o.stderr))),
        None => Err("tmux を起動できませんでした".into()),
    }
}

/// `#S:#I` of the dashboard's own window, for the header.
#[must_use]
pub fn own_window(client: &Client) -> Option<String> {
    let pane = client.pane.as_ref()?;
    let mut cmd = Command::new("tmux");
    cmd.args(["display-message", "-p", "-t", pane, "#S:#I"]);
    let out = crate::cmd::run(cmd, TMUX_TIMEOUT).filter(|o| o.success)?;
    Some(crate::text::clean(&out.stdout)).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn row() -> Row {
        let t = DateTime::parse_from_rfc3339("2026-01-05T12:00:00+09:00").unwrap();
        Row {
            number: Some(1),
            shown: Shown::Waiting,
            session_id: "s".into(),
            host: HostKind::Host,
            local: true,
            repo: "repo-a".into(),
            project: "demo".into(),
            worktree: "feat-x".into(),
            branch: Some("feat/x".into()),
            tmux: Some("work:2.1".into()),
            tmux_pane: Some("%3".into()),
            repo_path: Some("/work/repo-a".into()),
            worktree_path: Some("/work/repo-a/wt".into()),
            started_at: t,
            updated_at: t,
        }
    }

    fn inside() -> Client {
        Client {
            inside: true,
            pane: Some("%9".into()),
        }
    }

    #[test]
    fn jump_targets_the_pane_or_the_container_window() {
        let c = Config::default();
        assert_eq!(
            jump(&row(), &inside(), &c).unwrap(),
            ["switch-client", "-t", "%3"]
        );
        let mut r = row();
        r.local = false;
        r.host = HostKind::Container;
        assert_eq!(
            jump(&r, &inside(), &c).unwrap(),
            ["switch-client", "-t", "dev"]
        );
        r.host = HostKind::Host;
        assert!(jump(&r, &inside(), &c).is_err());
        let mut r = row();
        r.tmux_pane = None;
        assert!(jump(&r, &inside(), &c).unwrap_err().contains("tmux の外"));
        let mut r = row();
        r.shown = Shown::Stopped;
        assert!(jump(&r, &inside(), &c).is_err());
        assert!(jump(&row(), &Client::default(), &c).is_err());
    }

    #[test]
    fn diff_splits_beside_the_dashboard() {
        let base = "a".repeat(40);
        assert_eq!(
            diff(&row(), &inside(), Some(&base)).unwrap(),
            [
                "split-window",
                "-h",
                "-t",
                "%9",
                "-c",
                "/work/repo-a/wt",
                "hunk",
                "diff",
                &base,
                "--watch"
            ]
        );
        assert!(diff(&row(), &inside(), None).is_err());
        let mut r = row();
        r.worktree_path = None;
        assert!(diff(&r, &inside(), Some(&base)).is_err());
    }

    #[test]
    fn rename_uses_the_fitted_project_name() {
        assert_eq!(
            rename(&row(), &inside()).unwrap(),
            ["rename-window", "-t", "%3", "--", "demo"]
        );
        let mut r = row();
        r.project = "x".repeat(60);
        assert_eq!(crate::text::width(&rename(&r, &inside()).unwrap()[4]), 40);
        r.project = "-a todo".into();
        assert_eq!(rename(&r, &inside()).unwrap()[3..], ["--", "-a todo"]);
        r.project.clear();
        assert!(rename(&r, &inside()).is_err());
    }
}
