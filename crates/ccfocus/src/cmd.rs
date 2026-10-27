//! Bounded external commands for the dashboard (git, gh, tmux, kill).
//!
//! Unlike the hook's probe (tiny outputs, one shared budget), dashboard
//! commands can print a lot (`git status` of a busy tree), so stdout and
//! stderr are drained on their own threads while the child runs.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Bytes kept from each of stdout and stderr; the rest is read and dropped.
const MAX_OUTPUT: u64 = 4 * 1024 * 1024;

pub(crate) struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run `cmd` with no stdin. `None` when it can't be spawned or outlives
/// `timeout` (the child is then killed and its reader threads are left to
/// finish once the pipes close, so a grandchild holding a pipe can't hang
/// the caller).
pub(crate) fn run(mut cmd: Command, timeout: Duration) -> Option<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    let err = child.stderr.take()?;
    let out = std::thread::spawn(move || drain(out));
    let err = std::thread::spawn(move || drain(err));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some(Output {
        success: status.success(),
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// Start `cmd` for a command that may keep running (a tmux popup returns
/// only when it closes). `Ok(Some(output))` when it exits within `settle`,
/// `Ok(None)` when it is still running (a thread is left to wait for it),
/// `Err` when it can't be spawned or waited on.
pub(crate) fn start(mut cmd: Command, settle: Duration) -> Result<Option<Output>, ()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ())?;
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(());
    };
    let out = std::thread::spawn(move || drain(out));
    let err = std::thread::spawn(move || drain(err));
    let deadline = Instant::now() + settle;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Some(Output {
                    success: status.success(),
                    stdout: out.join().unwrap_or_default(),
                    stderr: err.join().unwrap_or_default(),
                }));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(None);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
        }
    }
}

fn drain(mut r: impl Read) -> String {
    let mut buf = Vec::new();
    let _ = r.by_ref().take(MAX_OUTPUT).read_to_end(&mut buf);
    let _ = std::io::copy(&mut r, &mut std::io::sink());
    String::from_utf8_lossy(&buf).into_owned()
}

/// Each git call; local plumbing finishes in milliseconds.
const GIT_TIMEOUT: Duration = Duration::from_secs(3);

/// `git -C dir <args>` stdout on success. Resolves from `dir` only and never
/// takes optional locks (the dashboard must not contend with the session's
/// own git work). `core.fsmonitor` is off: it runs a configured command on
/// `status`, and `dir` comes from a state file.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.fsmonitor=false", "-C"])
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_OPTIONAL_LOCKS", "0");
    run(cmd, GIT_TIMEOUT)
        .filter(|o| o.success)
        .map(|o| o.stdout)
}

/// First line of a failed command's stderr, cleaned for display.
pub(crate) fn first_line(stderr: &str) -> String {
    crate::text::clean(stderr.lines().next().unwrap_or_default())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_both_streams_and_status() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo out; echo err >&2; exit 3"]);
        let o = run(cmd, Duration::from_secs(5)).unwrap();
        assert!(!o.success);
        assert_eq!(o.stdout, "out\n");
        assert_eq!(o.stderr, "err\n");
    }

    #[test]
    fn large_output_does_not_deadlock() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes x | head -c 1000000"]);
        let o = run(cmd, Duration::from_secs(10)).unwrap();
        assert!(o.success);
        assert_eq!(o.stdout.len(), 1_000_000);
    }

    #[test]
    fn start_reports_a_quick_exit_and_leaves_a_long_one_running() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo err >&2; exit 1"]);
        let o = start(cmd, Duration::from_secs(5)).unwrap().unwrap();
        assert!(!o.success);
        assert_eq!(o.stderr, "err\n");
        let mut cmd = Command::new("sleep");
        cmd.arg("1");
        let begin = Instant::now();
        assert!(start(cmd, Duration::from_millis(100)).unwrap().is_none());
        assert!(begin.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn times_out() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let start = Instant::now();
        assert!(run(cmd, Duration::from_millis(100)).is_none());
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
