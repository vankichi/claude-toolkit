//! Event-by-event tests over the synthetic fixtures in `tests/fixtures`.

use super::*;
use crate::probe::{GitInfo, TmuxInfo};
use cctk::focus::HostKind;
use std::path::PathBuf;

const SID: &str = "00000000-0000-4000-8000-000000000001";

const START: &str = include_str!("../tests/fixtures/session_start.json");
const RESUME: &str = include_str!("../tests/fixtures/session_start_resume.json");
const PROMPT: &str = include_str!("../tests/fixtures/user_prompt_submit.json");
const PERMISSION: &str = include_str!("../tests/fixtures/notification_permission.json");
const IDLE: &str = include_str!("../tests/fixtures/notification_idle.json");
const STOP: &str = include_str!("../tests/fixtures/stop.json");
const END: &str = include_str!("../tests/fixtures/session_end.json");
const URL: &str = include_str!("../tests/fixtures/notification_url.json");
const BATCH: &str = include_str!("../tests/fixtures/post_tool_batch.json");
const FAILURE: &str = include_str!("../tests/fixtures/stop_failure.json");

struct FakeProbe;

impl Probe for FakeProbe {
    fn host(&self) -> HostKind {
        HostKind::Container
    }
    fn hostname(&self) -> Option<String> {
        Some("example-host".into())
    }
    fn tmux(&self) -> TmuxInfo {
        TmuxInfo {
            location: Some("work:2.1".into()),
            pane: Some("%7".into()),
        }
    }
    fn pid(&self) -> Option<u32> {
        Some(4242)
    }
    fn git(&self, cwd: &Path) -> GitInfo {
        assert_eq!(cwd, Path::new("/work/example"));
        GitInfo {
            repo: Some("/work/example".into()),
            worktree: Some("/work/example/.claude/worktrees/wt".into()),
            branch: Some("feat/x".into()),
        }
    }
}

fn at(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}

const T1: &str = "2026-01-02T03:04:05.000+09:00";
const T2: &str = "2026-01-02T03:10:00.000+09:00";

fn run(event: &str, input: &str, dir: &Path, now: &str) -> Result<Outcome> {
    handle(event, input.as_bytes(), dir, at(now), &FakeProbe)
}

fn state(dir: &Path) -> SessionState {
    focus::read_session(&focus::session_path(dir, SID)).expect("session file")
}

fn events(dir: &Path) -> String {
    std::fs::read_to_string(focus::events_dir(dir).join("20260102.tsv")).unwrap_or_default()
}

fn all_bytes_under(dir: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    for sub in [focus::sessions_dir(dir), focus::events_dir(dir)] {
        for e in std::fs::read_dir(sub).into_iter().flatten().flatten() {
            out.extend(std::fs::read(e.path()).unwrap());
        }
    }
    out
}

#[test]
fn session_start_writes_full_state_and_event() {
    let tmp = tempfile::tempdir().unwrap();
    let got = run("SessionStart", START, tmp.path(), T1).unwrap();
    assert_eq!(got, Outcome::Written(Status::Idle));
    assert_eq!(
        state(tmp.path()),
        SessionState {
            schema: SCHEMA,
            session_id: SID.into(),
            host: HostKind::Container,
            hostname: Some("example-host".into()),
            tmux: Some("work:2.1".into()),
            tmux_pane: Some("%7".into()),
            pid: Some(4242),
            cwd: "/work/example".into(),
            repo: Some("/work/example".into()),
            worktree: Some("/work/example/.claude/worktrees/wt".into()),
            branch: Some("feat/x".into()),
            project: Some("demo-focus".into()),
            status: Status::Idle,
            started_at: T1.into(),
            updated_at: T1.into(),
        }
    );
    assert_eq!(events(tmp.path()), format!("{T1}\t{SID}\tSessionStart\n"));
}

#[test]
fn lifecycle_maps_each_event_to_its_status() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    run("SessionStart", START, d, T1).unwrap();

    assert_eq!(
        run("UserPromptSubmit", PROMPT, d, T2).unwrap(),
        Outcome::Written(Status::Working)
    );
    assert_eq!(state(d).status, Status::Working);

    assert_eq!(
        run("Notification", PERMISSION, d, T2).unwrap(),
        Outcome::Written(Status::Waiting)
    );
    assert_eq!(state(d).status, Status::Waiting);

    assert_eq!(
        run("Stop", STOP, d, T2).unwrap(),
        Outcome::Written(Status::Done)
    );
    let s = state(d);
    assert_eq!(s.status, Status::Done);
    assert_eq!(s.started_at, T1);
    assert_eq!(s.updated_at, T2);
    // The title came from SessionStart; later events without one keep it.
    assert_eq!(s.project.as_deref(), Some("demo-focus"));

    assert_eq!(run("SessionEnd", END, d, T2).unwrap(), Outcome::Removed);
    assert!(!focus::session_path(d, SID).exists());
    assert_eq!(
        events(d),
        format!(
            "{T1}\t{SID}\tSessionStart\n{T2}\t{SID}\tUserPromptSubmit\n\
             {T2}\t{SID}\tNotification\n{T2}\t{SID}\tStop\n"
        )
    );
}

#[test]
fn other_notification_types_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        run("Notification", IDLE, tmp.path(), T1).unwrap(),
        Outcome::Ignored
    );
    assert!(!focus::sessions_dir(tmp.path()).exists());
    assert_eq!(events(tmp.path()), "");
}

#[test]
fn url_elicitation_waits() {
    let tmp = tempfile::tempdir().unwrap();
    run("SessionStart", START, tmp.path(), T1).unwrap();
    assert_eq!(
        run("Notification", URL, tmp.path(), T2).unwrap(),
        Outcome::Written(Status::Waiting)
    );
}

#[test]
fn post_tool_batch_ends_a_wait_only() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    run("SessionStart", START, d, T1).unwrap();
    run("UserPromptSubmit", PROMPT, d, T1).unwrap();
    run("Notification", PERMISSION, d, T1).unwrap();

    // The user approved; the tools ran and the batch resolved.
    assert_eq!(
        run("PostToolBatch", BATCH, d, T2).unwrap(),
        Outcome::Written(Status::Working)
    );
    let s = state(d);
    assert_eq!(s.status, Status::Working);
    assert_eq!(s.updated_at, T2);
    assert!(events(d).ends_with(&format!("{T2}\t{SID}\tPostToolBatch\n")));

    // Already working: nothing is rewritten or logged.
    let before = std::fs::read(focus::session_path(d, SID)).unwrap();
    let log = events(d);
    assert_eq!(
        run("PostToolBatch", BATCH, d, "2026-01-02T03:11:00.000+09:00").unwrap(),
        Outcome::Ignored
    );
    assert_eq!(std::fs::read(focus::session_path(d, SID)).unwrap(), before);
    assert_eq!(events(d), log);
}

#[test]
fn post_tool_batch_without_state_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("focus");
    assert_eq!(
        run("PostToolBatch", BATCH, &d, T1).unwrap(),
        Outcome::Ignored
    );
    assert!(!d.exists());
}

#[test]
fn stop_failure_marks_the_session_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    run("SessionStart", START, d, T1).unwrap();
    run("UserPromptSubmit", PROMPT, d, T1).unwrap();
    assert_eq!(
        run("StopFailure", FAILURE, d, T2).unwrap(),
        Outcome::Written(Status::Failed)
    );
    assert_eq!(state(d).status, Status::Failed);
    assert!(events(d).ends_with(&format!("{T2}\t{SID}\tStopFailure\n")));
    // The next prompt resumes work.
    run("UserPromptSubmit", PROMPT, d, T2).unwrap();
    assert_eq!(state(d).status, Status::Working);
}

#[test]
fn resume_keeps_started_at() {
    let tmp = tempfile::tempdir().unwrap();
    run("SessionStart", START, tmp.path(), T1).unwrap();
    run("SessionStart", RESUME, tmp.path(), T2).unwrap();
    let s = state(tmp.path());
    assert_eq!(s.started_at, T1);
    assert_eq!(s.updated_at, T2);
    assert_eq!(s.status, Status::Idle);
}

#[test]
fn resume_after_session_end_takes_start_from_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let (transcript, _) = with_transcript(
        tmp.path(),
        &[
            r#"{"type":"summary","summary":"no timestamp here"}"#,
            r#"{"type":"user","timestamp":"2026-01-01T15:00:00.250Z"}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T16:00:00.000Z"}"#,
        ],
    );
    let input = serde_json::json!({
        "session_id": SID,
        "transcript_path": transcript,
        "cwd": "/work/example",
        "source": "resume",
    })
    .to_string();
    let d = tmp.path().join("focus");
    // The real order: the first run ends (file removed), then it is resumed.
    run("SessionStart", START, &d, T1).unwrap();
    run("SessionEnd", END, &d, T1).unwrap();
    run("SessionStart", &input, &d, T2).unwrap();
    // 15:00 UTC in the hook's +09:00 offset.
    assert_eq!(state(&d).started_at, "2026-01-02T00:00:00.250+09:00");
}

#[test]
fn compaction_keeps_the_current_status() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    run("SessionStart", START, d, T1).unwrap();
    run("UserPromptSubmit", PROMPT, d, T1).unwrap();
    let compact = START.replace("\"startup\"", "\"compact\"");
    assert_eq!(
        run("SessionStart", &compact, d, T2).unwrap(),
        Outcome::Written(Status::Working)
    );
    assert_eq!(state(d).status, Status::Working);
    // A fresh SessionStart (no prior file) is still idle.
    let other = tempfile::tempdir().unwrap();
    run("SessionStart", &compact, other.path(), T2).unwrap();
    assert_eq!(state(other.path()).status, Status::Idle);
}

#[test]
fn an_older_hook_does_not_overwrite_a_newer_state() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    run("SessionStart", START, d, T1).unwrap();
    // The Notification hook started (and wrote) after the prompt hook began.
    run("Notification", PERMISSION, d, T2).unwrap();
    // The slower UserPromptSubmit, stamped earlier, finishes last.
    run(
        "UserPromptSubmit",
        PROMPT,
        d,
        "2026-01-02T03:09:59.900+09:00",
    )
    .unwrap();
    let s = state(d);
    assert_eq!(s.status, Status::Waiting);
    assert_eq!(s.updated_at, T2);
    // Its event is still logged.
    assert!(events(d).ends_with("\tUserPromptSubmit\n"));
}

#[test]
fn session_end_without_state_dir_is_a_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("focus");
    assert_eq!(run("SessionEnd", END, &dir, T1).unwrap(), Outcome::Removed);
    assert!(!dir.exists());
}

#[test]
fn bodies_are_never_written() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for (event, input) in [
        ("SessionStart", START),
        ("UserPromptSubmit", PROMPT),
        ("Notification", PERMISSION),
        // Written: the session is waiting, so the batch ends the wait.
        ("PostToolBatch", BATCH),
        ("Notification", URL),
        ("Notification", IDLE),
        ("StopFailure", FAILURE),
        ("Stop", STOP),
    ] {
        run(event, input, d, T1).unwrap();
    }
    let written = String::from_utf8(all_bytes_under(d)).unwrap();
    assert!(written.contains("\tPostToolBatch\n"));
    for body in [
        "SYNTHETIC-PROMPT-BODY",
        "SYNTHETIC-MESSAGE-BODY",
        "SYNTHETIC-ASSISTANT-BODY",
        "SYNTHETIC-TOOL-INPUT",
        "SYNTHETIC-TOOL-OUTPUT",
        "SYNTHETIC-ERROR-DETAILS",
        "rate_limit",
        "Permission needed",
    ] {
        assert!(!written.contains(body), "{body} leaked into state");
    }
}

fn with_transcript(dir: &Path, lines: &[&str]) -> (PathBuf, String) {
    let transcript = dir.join("transcript.jsonl");
    std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
    let input = serde_json::json!({
        "session_id": SID,
        "transcript_path": transcript,
        "cwd": "/work/example",
        "hook_event_name": "SessionStart",
        "source": "startup",
    })
    .to_string();
    (transcript, input)
}

#[test]
fn project_falls_back_to_last_custom_title() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, input) = with_transcript(
        tmp.path(),
        &[
            r#"{"type":"custom-title","customTitle":"first-name","sessionId":"x"}"#,
            r#"{"type":"user","message":{"content":"mentions \"custom-title\" in text"}}"#,
            r#"{"type":"custom-title","customTitle":"renamed","sessionId":"x"}"#,
        ],
    );
    let state_dir = tmp.path().join("focus");
    run("SessionStart", &input, &state_dir, T1).unwrap();
    assert_eq!(state(&state_dir).project.as_deref(), Some("renamed"));
}

#[test]
fn prompt_session_title_renames_project() {
    let tmp = tempfile::tempdir().unwrap();
    run("SessionStart", START, tmp.path(), T1).unwrap();
    let renamed = PROMPT.replace(
        "\"permission_mode\"",
        "\"session_title\":\"renamed\",\"permission_mode\"",
    );
    run("UserPromptSubmit", &renamed, tmp.path(), T2).unwrap();
    assert_eq!(state(tmp.path()).project.as_deref(), Some("renamed"));
}

#[test]
fn project_drops_control_characters() {
    assert_eq!(
        clean_project("demo\u{1b}[31m-x\n").as_deref(),
        Some("demo[31m-x")
    );
    assert_eq!(clean_project(" \u{7} "), None);
    assert_eq!(
        clean_project("a\u{202E}b\u{200B}c\u{2066}d\u{FEFF}").as_deref(),
        Some("abcd")
    );
    assert_eq!(clean_project(&"a".repeat(500)).unwrap().len(), 200);
}

#[test]
fn rejects_broken_json_unknown_events_and_bad_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    assert!(run("SessionStart", "{not json", d, T1).is_err());
    assert!(run("PreToolUse", START, d, T1).is_err());
    let traversal = START.replace(SID, "../escape");
    assert!(run("SessionStart", &traversal, d, T1).is_err());
    assert!(!d.join("escape.json").exists());
    assert!(!focus::sessions_dir(d).exists());
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn mode(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn dirs_are_0700_and_files_0600() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("focus");
        run("SessionStart", START, &d, T1).unwrap();
        assert_eq!(mode(&d), 0o700);
        assert_eq!(mode(&focus::sessions_dir(&d)), 0o700);
        assert_eq!(mode(&focus::events_dir(&d)), 0o700);
        assert_eq!(mode(&focus::session_path(&d, SID)), 0o600);
        assert_eq!(mode(&focus::events_dir(&d).join("20260102.tsv")), 0o600);
    }

    #[test]
    fn widened_modes_are_tightened_except_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("focus");
        run("SessionStart", START, &d, T1).unwrap();
        let sessions = focus::sessions_dir(&d);
        let log = focus::events_dir(&d).join("20260102.tsv");
        for (p, m) in [(&d, 0o755), (&sessions, 0o755), (&log, 0o644)] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
        }
        run("Stop", STOP, &d, T2).unwrap();
        // The root may be a directory the user chose; it is not chmodded.
        assert_eq!(mode(&d), 0o755);
        assert_eq!(mode(&sessions), 0o700);
        assert_eq!(mode(&log), 0o600);
    }

    #[test]
    fn symlinked_state_dir_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("elsewhere");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("focus");
        symlink(&real, &link).unwrap();
        assert!(run("SessionStart", START, &link, T1).is_err());
        assert!(run("SessionEnd", END, &link, T1).is_err());
        assert_eq!(std::fs::read_dir(&real).unwrap().count(), 0);
    }

    #[test]
    fn symlinked_session_file_is_replaced_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("focus");
        run("SessionStart", START, &d, T1).unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "keep me").unwrap();
        let path = focus::session_path(&d, SID);
        std::fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();

        run("Stop", STOP, &d, T2).unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me");
        assert!(std::fs::symlink_metadata(&path).unwrap().is_file());
        assert_eq!(state(&d).status, Status::Done);

        // SessionEnd removes a link itself, never its target.
        std::fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        run("SessionEnd", END, &d, T2).unwrap();
        assert!(std::fs::symlink_metadata(&path).is_err());
        assert!(outside.exists());
    }

    #[test]
    fn symlinked_sessions_or_events_dir_is_refused() {
        for sub in ["sessions", "events"] {
            let tmp = tempfile::tempdir().unwrap();
            let d = tmp.path().join("focus");
            std::fs::create_dir(&d).unwrap();
            let outside = tmp.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            symlink(&outside, d.join(sub)).unwrap();

            assert!(run("SessionStart", START, &d, T1).is_err(), "{sub}");
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "{sub}");
        }
    }

    #[test]
    fn symlinked_event_log_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("focus");
        run("SessionStart", START, &d, T1).unwrap();
        let outside = tmp.path().join("outside.tsv");
        std::fs::write(&outside, "keep me").unwrap();
        let log = focus::events_dir(&d).join("20260102.tsv");
        std::fs::remove_file(&log).unwrap();
        symlink(&outside, &log).unwrap();

        assert!(run("Stop", STOP, &d, T2).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me");
    }

    #[test]
    fn symlinked_transcript_is_not_read() {
        let tmp = tempfile::tempdir().unwrap();
        let (transcript, input) = with_transcript(
            tmp.path(),
            &[r#"{"type":"custom-title","customTitle":"via-link"}"#],
        );
        let link = tmp.path().join("link.jsonl");
        symlink(&transcript, &link).unwrap();
        let input = input.replace(transcript.to_str().unwrap(), link.to_str().unwrap());
        let d = tmp.path().join("focus");
        run("SessionStart", &input, &d, T1).unwrap();
        assert_eq!(state(&d).project, None);
    }
}
