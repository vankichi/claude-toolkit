//! The dashboard's key actions as tmux commands. Planning is pure (an
//! argument list or the reason it can't be done); [`run`] executes one.
//! Arguments go to tmux directly, never through a shell. tmux itself
//! format-expands some of them (the `rename-window` name and the
//! `display-popup -d` directory), where `#(…)` would run a shell command, so
//! values from the state files are passed through [`literal`] there.

use crate::board::{Row, Shown};
use crate::config::Config;
use cctk::focus::HostKind;
use std::process::Command;
use std::time::Duration;

/// Longest window name `p` sets.
const MAX_WINDOW_NAME: usize = 40;

const TMUX_TIMEOUT: Duration = Duration::from_secs(2);

/// How long [`open`] waits for tmux to reject a popup (a bad target fails
/// in milliseconds).
const POPUP_SETTLE: Duration = Duration::from_millis(300);

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
        Err("run the board inside tmux to use this key".into())
    }
}

/// Switch to the session's pane. A container session (whose pane lives in
/// the host tmux, in the window that runs the container) goes to
/// `config.container_target`.
pub fn jump(row: &Row, client: &Client, config: &Config) -> Plan {
    need_tmux(client)?;
    if row.shown == Shown::Stopped {
        return Err("that session has stopped".into());
    }
    if !row.local {
        return if row.host == HostKind::Container {
            Ok(args(&["switch-client", "-t", &config.container_target]))
        } else {
            Err("that session runs on the other side (host / container)".into())
        };
    }
    match &row.tmux_pane {
        Some(pane) => Ok(args(&["switch-client", "-t", pane])),
        None => Err("that session is not in tmux".into()),
    }
}

/// Open `hunk diff <merge-base> --watch` in a popup over the dashboard, in
/// the session's worktree; it closes when hunk exits. A popup leaves the
/// window's layout alone (a split's space would not return to the board).
/// `base` is the merge-base the caller computed. Run it with [`open`].
pub fn diff(row: &Row, client: &Client, base: Option<&str>) -> Plan {
    need_tmux(client)?;
    if !row.local {
        return Err("can't open a worktree on the other side (host / container)".into());
    }
    let Some(worktree) = row.worktree_path.as_ref().and_then(|p| p.to_str()) else {
        return Err("that session is not in a git worktree".into());
    };
    let Some(base) = base else {
        return Err("no merge-base with origin/HEAD".into());
    };
    let mut a = args(&["display-popup", "-E", "-w", "90%", "-h", "90%"]);
    if let Some(pane) = &client.pane {
        a.extend(args(&["-t", pane]));
    }
    // Two or more command arguments run as argv, without a shell.
    a.extend(args(&[
        "-d",
        &literal(worktree),
        "hunk",
        "diff",
        base,
        "--watch",
    ]));
    Ok(a)
}

/// Name the session's tmux window after its project.
pub fn rename(row: &Row, client: &Client) -> Plan {
    need_tmux(client)?;
    if !row.local {
        return Err("that session runs on the other side (host / container)".into());
    }
    let Some(pane) = &row.tmux_pane else {
        return Err("that session is not in tmux".into());
    };
    if row.project.is_empty() {
        return Err("no project name (set one with claude -n)".into());
    }
    let name = literal(&crate::text::fit(&row.project, MAX_WINDOW_NAME));
    // `--`: a project like `-a todo` must not be read as flags.
    Ok(args(&["rename-window", "-t", pane, "--", &name]))
}

/// `s` as a tmux format that expands to itself: every `#` doubled, so
/// `#(cmd)`, `#{…}`, and `#H` stay text.
#[must_use]
pub fn literal(s: &str) -> String {
    s.replace('#', "##")
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
        None => Err("couldn't run tmux".into()),
    }
}

/// Run `tmux <args>` for a command that stays until it is closed
/// (`display-popup` returns only then). An error tmux reports at once is
/// returned; a command still running after [`POPUP_SETTLE`] is open.
pub fn open(args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("tmux");
    cmd.args(args);
    match crate::cmd::start(cmd, POPUP_SETTLE) {
        Ok(None) => Ok(()),
        Ok(Some(o)) if o.success => Ok(()),
        Ok(Some(o)) => Err(format!("tmux: {}", crate::cmd::first_line(&o.stderr))),
        Err(()) => Err("couldn't run tmux".into()),
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
        assert!(jump(&r, &inside(), &c).unwrap_err().contains("not in tmux"));
        let mut r = row();
        r.shown = Shown::Stopped;
        assert!(jump(&r, &inside(), &c).is_err());
        assert!(jump(&row(), &Client::default(), &c).is_err());
    }

    #[test]
    fn diff_opens_a_popup_over_the_dashboard() {
        let base = "a".repeat(40);
        assert_eq!(
            diff(&row(), &inside(), Some(&base)).unwrap(),
            [
                "display-popup",
                "-E",
                "-w",
                "90%",
                "-h",
                "90%",
                "-t",
                "%9",
                "-d",
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
    fn values_tmux_expands_are_escaped() {
        let mut r = row();
        r.project = "a #(touch x) #H".into();
        assert_eq!(rename(&r, &inside()).unwrap()[4], "a ##(touch x) ##H");
        r.worktree_path = Some("/w/#(x)".into());
        let plan = diff(&r, &inside(), Some(&"a".repeat(40))).unwrap();
        let d = plan.iter().position(|a| a == "-d").unwrap();
        assert_eq!(plan[d + 1], "/w/##(x)");
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
