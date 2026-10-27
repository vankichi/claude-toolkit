//! Facts about the running Claude process that the hook input doesn't carry:
//! host vs container, hostname, tmux pane, parent pid, and git location.
//!
//! [`Probe`] is a trait so tests can inject fixed values; [`SystemProbe`]
//! asks the OS. Every external command runs with a timeout, and any failure
//! degrades to `None` rather than an error.

use cctk::focus::HostKind;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Upper bound for each external command (git, tmux, hostname), within the
/// probe's overall budget. The hook blocks prompt submission while it runs,
/// so a hung command must not stall it.
const COMMAND_TIMEOUT: Duration = Duration::from_millis(500);

/// Where the session's working directory sits in git.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GitInfo {
    pub repo: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
}

/// tmux location of the session's pane.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TmuxInfo {
    /// `#S:#I.#P`
    pub location: Option<String>,
    /// `$TMUX_PANE` (`%N`)
    pub pane: Option<String>,
}

pub trait Probe {
    fn host(&self) -> HostKind;
    fn hostname(&self) -> Option<String>;
    fn tmux(&self) -> TmuxInfo;
    fn pid(&self) -> Option<u32>;
    fn git(&self, cwd: &Path) -> GitInfo;
}

/// Probe backed by the environment, the OS, and the `git` / `tmux` /
/// `hostname` commands, all sharing one time budget.
pub struct SystemProbe {
    budget_end: Instant,
}

impl SystemProbe {
    /// A probe whose external commands together may take at most `budget`.
    #[must_use]
    pub fn new(budget: Duration) -> Self {
        Self {
            budget_end: Instant::now() + budget,
        }
    }

    fn run(&self, cmd: Command) -> Option<String> {
        run(cmd, self.budget_end)
    }
}

impl Probe for SystemProbe {
    fn host(&self) -> HostKind {
        if std::env::var_os("DEV_CONTAINER").is_some_and(|v| !v.is_empty()) {
            HostKind::Container
        } else {
            HostKind::Host
        }
    }

    fn hostname(&self) -> Option<String> {
        if let Ok(name) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
            return non_empty(name.trim());
        }
        self.run(Command::new("hostname"))
            .and_then(|out| non_empty(out.trim()))
    }

    fn tmux(&self) -> TmuxInfo {
        let Some(pane) = std::env::var("TMUX_PANE").ok().filter(|p| is_pane_id(p)) else {
            return TmuxInfo::default();
        };
        let mut cmd = Command::new("tmux");
        cmd.args(["display-message", "-p", "-t", &pane, "#S:#I.#P"]);
        TmuxInfo {
            location: self.run(cmd).and_then(|out| non_empty(out.trim())),
            pane: Some(pane),
        }
    }

    fn pid(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            Some(std::os::unix::process::parent_id())
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    fn git(&self, cwd: &Path) -> GitInfo {
        let rev_parse = |args: &[&str]| {
            let mut cmd = Command::new("git");
            cmd.arg("-C")
                .arg(cwd)
                .args(["rev-parse", "--path-format=absolute"])
                .args(args)
                // Resolve from cwd only, and never take optional locks.
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env("GIT_OPTIONAL_LOCKS", "0");
            self.run(cmd)
        };
        // Usually one call. An unborn branch makes `HEAD` ambiguous, and a bare
        // repo or a cwd inside `.git` has no toplevel; each failure drops the
        // last query so the remaining fields are still filled.
        rev_parse(&[
            "--git-common-dir",
            "--show-toplevel",
            "--symbolic-full-name",
            "HEAD",
        ])
        .or_else(|| rev_parse(&["--git-common-dir", "--show-toplevel"]))
        .or_else(|| rev_parse(&["--git-common-dir"]))
        .map(|o| parse_rev_parse(&o))
        .unwrap_or_default()
    }
}

/// Parse `rev-parse --git-common-dir --show-toplevel [--symbolic-full-name HEAD]`
/// output, one value per line in that order.
fn parse_rev_parse(out: &str) -> GitInfo {
    // git before 2.31 doesn't know `--path-format`: it echoes the flag back
    // as the first line and exits 0, which would shift every field. A real
    // first line is a rooted path (`/…`, or `C:/…` on Windows).
    if !out
        .lines()
        .next()
        .is_some_and(|l| Path::new(l.trim()).has_root())
    {
        return GitInfo::default();
    }
    let mut lines = out.lines().map(str::trim);
    let common = lines.next().and_then(non_empty);
    let worktree = lines.next().and_then(non_empty);
    let branch = lines
        .next()
        .and_then(|r| r.strip_prefix("refs/heads/"))
        .and_then(non_empty);
    let repo = common.map(|c| {
        let p = Path::new(&c);
        // `<checkout>/.git` → `<checkout>`; a bare repo's dir is the repo.
        if p.file_name().is_some_and(|n| n == ".git") {
            p.parent()
                .map_or(c.clone(), |parent| parent.to_string_lossy().into_owned())
        } else {
            c
        }
    });
    GitInfo {
        repo,
        worktree,
        branch,
    }
}

/// `%` followed by digits, as tmux sets `$TMUX_PANE`.
#[must_use]
pub fn is_pane_id(s: &str) -> bool {
    s.strip_prefix('%')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Run `cmd` with no stdin and stderr discarded; `Some(stdout)` on a zero
/// exit within [`COMMAND_TIMEOUT`] and before `budget_end`, else `None` (the
/// child is killed on timeout; nothing is spawned once the budget is spent).
fn run(mut cmd: Command, budget_end: Instant) -> Option<String> {
    let deadline = budget_end.min(Instant::now() + COMMAND_TIMEOUT);
    if Instant::now() >= deadline {
        return None;
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktree_rev_parse() {
        let got = parse_rev_parse(
            "/work/example/.git\n/work/example/.claude/worktrees/wt\nrefs/heads/feat/x\n",
        );
        assert_eq!(
            got,
            GitInfo {
                repo: Some("/work/example".into()),
                worktree: Some("/work/example/.claude/worktrees/wt".into()),
                branch: Some("feat/x".into()),
            }
        );
    }

    #[test]
    fn output_of_a_git_without_path_format_is_rejected() {
        // git < 2.31 echoes the unknown flag, then prints relative paths.
        let got = parse_rev_parse("--path-format=absolute\n.git\n/work/example\nrefs/heads/main\n");
        assert_eq!(got, GitInfo::default());
        assert_eq!(parse_rev_parse(""), GitInfo::default());
    }

    #[test]
    fn detached_head_has_no_branch() {
        let got = parse_rev_parse("/work/example/.git\n/work/example\nHEAD\n");
        assert_eq!(got.branch, None);
        assert_eq!(got.repo.as_deref(), Some("/work/example"));
    }

    #[test]
    fn parses_a_non_dotgit_common_dir_as_the_repo() {
        let got = parse_rev_parse("/work/example.git\n");
        assert_eq!(got.repo.as_deref(), Some("/work/example.git"));
        assert_eq!(got.worktree, None);
    }

    #[test]
    fn a_spent_budget_spawns_nothing() {
        let probe = SystemProbe::new(Duration::ZERO);
        let tmp = tempfile::tempdir().unwrap();
        let started = Instant::now();
        assert_eq!(probe.git(tmp.path()), GitInfo::default());
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_command_is_killed_at_the_deadline() {
        let started = Instant::now();
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        assert_eq!(run(cmd, Instant::now() + Duration::from_millis(100)), None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn pane_ids() {
        assert!(is_pane_id("%12"));
        assert!(!is_pane_id("%"));
        assert!(!is_pane_id("12"));
        assert!(!is_pane_id("%1;rm"));
    }

    #[test]
    fn git_outside_a_repo_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        // A temp dir is normally outside any repo; if the runner nests TMPDIR
        // in one, at least the call must not fail.
        let got = SystemProbe::new(Duration::from_secs(1)).git(tmp.path());
        if got.repo.is_none() {
            assert_eq!(got, GitInfo::default());
        }
    }
}
