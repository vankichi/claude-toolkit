//! Process-level contract of `ccfocus hook`: exit 0 on every failure, nothing
//! on stdout, at most one stderr line.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const START: &str = include_str!("fixtures/session_start.json");
const END: &str = include_str!("fixtures/session_end.json");
const PERMISSION: &str = include_str!("fixtures/notification_permission.json");
const BATCH: &str = include_str!("fixtures/post_tool_batch.json");
const FAILURE: &str = include_str!("fixtures/stop_failure.json");

fn ccfocus(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ccfocus"))
        .args(args)
        // Keep the host's tmux / container markers out of the run.
        .env_remove("TMUX_PANE")
        .env_remove("DEV_CONTAINER")
        .env_remove("CCFOCUS_STATE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn hook(event: &str, dir: &Path, stdin: &str) -> Output {
    ccfocus(
        &["hook", event, "--state-dir", dir.to_str().unwrap()],
        stdin,
    )
}

fn assert_quiet_success(out: &Output, expect_stderr: bool) {
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "stdout must stay empty");
    let stderr = String::from_utf8_lossy(&out.stderr);
    if expect_stderr {
        assert_eq!(stderr.lines().count(), 1, "one stderr line: {stderr}");
    } else {
        assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    }
}

#[test]
fn records_and_removes_a_session() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("focus");
    assert_quiet_success(&hook("SessionStart", &dir, START), false);
    let path = dir
        .join("sessions")
        .join("00000000-0000-4000-8000-000000000001.json");
    let state: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(state["status"], "idle");
    assert_eq!(state["project"], "demo-focus");

    assert_quiet_success(&hook("SessionEnd", &dir, END), false);
    assert!(!path.exists());
}

#[test]
fn waits_end_on_a_tool_batch_and_api_errors_mark_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("focus");
    let status = || -> serde_json::Value {
        let path = dir
            .join("sessions")
            .join("00000000-0000-4000-8000-000000000001.json");
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()
            ["status"]
            .clone()
    };
    assert_quiet_success(&hook("SessionStart", &dir, START), false);
    assert_quiet_success(&hook("Notification", &dir, PERMISSION), false);
    assert_eq!(status(), "waiting");
    assert_quiet_success(&hook("PostToolBatch", &dir, BATCH), false);
    assert_eq!(status(), "working");
    assert_quiet_success(&hook("StopFailure", &dir, FAILURE), false);
    assert_eq!(status(), "failed");
}

#[test]
fn broken_json_still_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let out = hook("SessionStart", tmp.path(), "{\"session_id\": ");
    assert_quiet_success(&out, true);
}

#[test]
fn unknown_event_and_bad_args_exit_zero() {
    let tmp = tempfile::tempdir().unwrap();
    assert_quiet_success(&hook("PreToolUse", tmp.path(), START), true);
    assert_quiet_success(&ccfocus(&["hook"], ""), true);
    assert_quiet_success(&ccfocus(&["bogus"], ""), true);
}

#[test]
fn empty_state_dir_env_falls_back_to_default() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ccfocus"))
        .args(["hook", "SessionStart"])
        .env("CCFOCUS_STATE_DIR", "")
        .env("HOME", tmp.path())
        .env("USERPROFILE", tmp.path())
        .env_remove("TMUX_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().unwrap().write_all(START.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert_quiet_success(&out, false);
    assert!(tmp.path().join(".claude/focus/sessions").is_dir());
}

#[test]
fn unwritable_state_dir_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("not-a-dir");
    std::fs::write(&file, "").unwrap();
    assert_quiet_success(&hook("SessionStart", &file, START), true);
}
