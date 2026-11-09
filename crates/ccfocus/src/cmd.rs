//! Bounded external commands for the dashboard (git, gh, tmux, kill).
//!
//! Unlike the hook's probe (tiny outputs, one shared budget), dashboard
//! commands can print a lot (`git status` of a busy tree), so stdout and
//! stderr are drained on their own threads while the child runs.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Bytes kept from each of stdout and stderr; the rest is read and dropped.
const MAX_OUTPUT: u64 = 4 * 1024 * 1024;

/// Least time left to finish reading after the child exits, so a child that
/// exits right at the deadline doesn't lose output still in the pipes.
const READ_GRACE: Duration = Duration::from_millis(100);

pub(crate) struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run `cmd` with no stdin. `None` when it can't be spawned, outlives
/// `timeout` (it is then killed), or exits but its output isn't fully read
/// by then: a grandchild that keeps a pipe open (e.g. something a launcher
/// script started in the background) would otherwise hang the caller. The
/// reader threads are left to finish once the pipes close.
pub(crate) fn run(mut cmd: Command, timeout: Duration) -> Option<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    let err = child.stderr.take()?;
    let out = reader(out);
    let err = reader(err);
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
    let deadline = deadline.max(Instant::now() + READ_GRACE);
    Some(Output {
        success: status.success(),
        stdout: collect(&out, deadline)?,
        stderr: collect(&err, deadline)?,
    })
}

/// Start `cmd` for a command that may keep running (a tmux popup returns
/// only when it closes). `Ok(Some(output))` when it exits within `settle`,
/// `Ok(None)` when it is still running (a thread is left to wait for it),
/// `Err` when it can't be spawned or waited on. Output still unread by
/// `settle` (a grandchild holding a pipe) comes back empty.
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
    let out = reader(out);
    let err = reader(err);
    let deadline = Instant::now() + settle;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let deadline = deadline.max(Instant::now() + READ_GRACE);
                return Ok(Some(Output {
                    success: status.success(),
                    stdout: collect(&out, deadline).unwrap_or_default(),
                    stderr: collect(&err, deadline).unwrap_or_default(),
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

/// Drain `r` on its own thread; the text arrives on the returned channel
/// when the pipe closes.
fn reader(r: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // The caller may have stopped waiting; then nobody receives this.
        let _ = tx.send(drain(r));
    });
    rx
}

/// The text from a [`reader`], or `None` if it isn't done by `deadline`.
fn collect(rx: &Receiver<String>, deadline: Instant) -> Option<String> {
    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()
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

    /// The child exits at once, but a background grandchild keeps stdout
    /// open: reading must give up at the deadline, not wait for it.
    fn exits_but_leaves_a_pipe_open() -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 5 & echo started"]);
        cmd
    }

    #[test]
    fn a_grandchild_holding_a_pipe_does_not_hang_run() {
        let begin = Instant::now();
        assert!(run(exits_but_leaves_a_pipe_open(), Duration::from_millis(300)).is_none());
        assert!(begin.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_grandchild_holding_a_pipe_does_not_hang_start() {
        let begin = Instant::now();
        let o = start(exits_but_leaves_a_pipe_open(), Duration::from_millis(300))
            .unwrap()
            .unwrap();
        assert!(o.success);
        assert!(begin.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn collect_takes_finished_output_even_past_the_deadline() {
        let rx = reader(&b"done"[..]);
        let past = Instant::now();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(collect(&rx, past).as_deref(), Some("done"));
        let (_tx, never) = std::sync::mpsc::channel::<String>();
        assert_eq!(collect(&never, past), None);
    }
}
