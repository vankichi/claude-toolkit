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

/// How long to keep reading after the child exits. Output is drained while
/// the child runs, so only what is left in the pipes remains; a grandchild
/// holding a pipe open is given up on after this, however much of the
/// timeout is left.
const READ_WINDOW: Duration = Duration::from_millis(500);

/// What [`start`] gives as stderr when it couldn't read it in time.
const STDERR_UNREAD: &str = "stderr not read in time after it exited";

pub(crate) struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run `cmd` with no stdin. `None` when it can't be spawned, outlives
/// `timeout` (it is then killed), or exits but its output isn't fully read
/// within [`READ_WINDOW`]: a grandchild that keeps a pipe open (e.g.
/// something a launcher script started in the background) would otherwise
/// hang the caller. That case reads as a failure even if the command
/// itself succeeded, rather than handing back partial output. The reader
/// threads are left to finish once the pipes close.
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
    let (stdout, stderr) = read_after_exit(&out, &err);
    Some(Output {
        success: status.success(),
        stdout: stdout?,
        stderr: stderr?,
    })
}

/// Start `cmd` for a command that may keep running (a tmux popup returns
/// only when it closes). `Ok(Some(output))` when it exits within `settle`,
/// `Ok(None)` when it is still running (a thread is left to wait for it),
/// `Err` when it can't be spawned or waited on. A stream still unread
/// [`READ_WINDOW`] after the exit (a grandchild holding it) comes back
/// empty for stdout and as [`STDERR_UNREAD`] for stderr, so a failure
/// still says why it has no message.
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
                let (stdout, stderr) = read_after_exit(&out, &err);
                return Ok(Some(Output {
                    success: status.success(),
                    stdout: stdout.unwrap_or_default(),
                    stderr: stderr.unwrap_or_else(|| STDERR_UNREAD.to_string()),
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

/// stdout and stderr of a child that has just exited, each `None` if its
/// reader isn't done within [`READ_WINDOW`] (one window for both).
fn read_after_exit(
    out: &Receiver<String>,
    err: &Receiver<String>,
) -> (Option<String>, Option<String>) {
    let deadline = Instant::now() + READ_WINDOW;
    (collect(out, deadline), collect(err, deadline))
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
    /// open for 3 s. Waiting for the pipes would take those 3 s.
    fn exits_but_leaves_a_pipe_open() -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 3 & echo started"]);
        cmd
    }

    /// Well under the grandchild's 3 s, and over [`READ_WINDOW`] plus slack.
    const GIVES_UP_BY: Duration = Duration::from_secs(2);

    #[test]
    fn a_grandchild_holding_a_pipe_does_not_hang_run() {
        // A long timeout: after the exit only READ_WINDOW is spent, not the
        // rest of the timeout.
        let begin = Instant::now();
        assert!(run(exits_but_leaves_a_pipe_open(), Duration::from_secs(10)).is_none());
        assert!(begin.elapsed() < GIVES_UP_BY, "{:?}", begin.elapsed());
    }

    #[test]
    fn a_grandchild_holding_a_pipe_does_not_hang_start() {
        let begin = Instant::now();
        let o = start(exits_but_leaves_a_pipe_open(), Duration::from_millis(300))
            .unwrap()
            .unwrap();
        assert!(o.success);
        assert_eq!(o.stderr, STDERR_UNREAD);
        assert!(begin.elapsed() < GIVES_UP_BY, "{:?}", begin.elapsed());
    }

    #[test]
    fn collect_takes_finished_output_even_past_the_deadline() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send("done".to_string()).unwrap();
        let past = Instant::now();
        assert_eq!(collect(&rx, past).as_deref(), Some("done"));
        let (_tx, never) = std::sync::mpsc::channel::<String>();
        assert_eq!(collect(&never, past), None);
    }

    #[test]
    fn read_after_exit_waits_for_readers_still_finishing() {
        // A reader that finishes a little after the exit still counts.
        let (tx, out) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let _ = tx.send("late".to_string());
        });
        let (etx, err) = std::sync::mpsc::channel();
        etx.send(String::new()).unwrap();
        assert_eq!(
            read_after_exit(&out, &err),
            (Some("late".to_string()), Some(String::new()))
        );
    }
}
