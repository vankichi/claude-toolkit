//! Schema and read-only access for the ccfocus session-state files.
//!
//! `ccfocus hook <event>` (the only writer) keeps one JSON file per live
//! Claude Code session under `<state>/sessions/<session_id>.json` and appends
//! one line per event to `<state>/events/<YYYYMMDD>.tsv`. Readers (the ccfocus
//! TUI, cctop's Focus panel) use the types and helpers here, so the file shape
//! has a single source of truth. Nothing in this module writes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Version of the [`SessionState`] file shape. Bump on incompatible changes so
/// readers can skip files they don't understand.
pub const SCHEMA: u32 = 1;

/// Environment variable that overrides the default state directory.
pub const STATE_DIR_ENV: &str = "CCFOCUS_STATE_DIR";

/// What the session is doing, as last reported by a hook event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Started (`SessionStart`) and waiting for the first prompt.
    Idle,
    /// A prompt was submitted and Claude is working on it.
    Working,
    /// Claude is blocked on the user (permission prompt, elicitation, input).
    Waiting,
    /// Claude finished its turn (`Stop`).
    Done,
    /// The turn ended on an API error (`StopFailure`: rate limit, overload,
    /// authentication, …); Claude is stopped and needs the user to resume.
    Failed,
}

/// Where the Claude process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    Host,
    Container,
}

/// One `<state>/sessions/<session_id>.json` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub schema: u32,
    pub session_id: String,
    pub host: HostKind,
    pub hostname: Option<String>,
    /// `#S:#I.#P` of the session's tmux pane, when run inside tmux.
    pub tmux: Option<String>,
    /// `$TMUX_PANE` (`%N`), stable across window renumbering.
    pub tmux_pane: Option<String>,
    /// Parent process of the hook (the Claude process when the hook is
    /// registered without a wrapping shell). In a container this is a
    /// container-namespace pid.
    pub pid: Option<u32>,
    pub cwd: String,
    /// Main checkout: the parent of `git rev-parse --git-common-dir` when that
    /// is a `.git` directory, else the common dir itself (bare repo, submodule,
    /// separate git dir).
    pub repo: Option<String>,
    /// Checked-out worktree: `git rev-parse --show-toplevel`.
    pub worktree: Option<String>,
    /// Current branch; `None` when detached or outside git.
    pub branch: Option<String>,
    /// `claude -n` name: the hook's `session_title`, else the transcript's
    /// last `custom-title`.
    pub project: Option<String>,
    pub status: Status,
    /// RFC 3339, local offset.
    pub started_at: String,
    /// RFC 3339, local offset.
    pub updated_at: String,
}

/// Default state directory: `$CCFOCUS_STATE_DIR`, else `~/.claude/focus`.
#[must_use]
pub fn default_state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(STATE_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::home_dir().map(|h| h.join(".claude").join("focus"))
}

#[must_use]
pub fn sessions_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("sessions")
}

#[must_use]
pub fn events_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("events")
}

/// True for ids safe to use as a file name: 1–128 of `[A-Za-z0-9_-]`.
#[must_use]
pub fn is_valid_session_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// File name of the state file for `session_id` (not validated; see
/// [`is_valid_session_id`]).
#[must_use]
pub fn session_file_name(session_id: &str) -> String {
    format!("{session_id}.json")
}

/// Path of the state file for `session_id` (not validated; see
/// [`is_valid_session_id`]).
#[must_use]
pub fn session_path(state_dir: &Path, session_id: &str) -> PathBuf {
    sessions_dir(state_dir).join(session_file_name(session_id))
}

/// State files are a few hundred bytes; anything larger than this is not one
/// (and dashboards read every file every few seconds).
pub const MAX_SESSION_BYTES: u64 = 64 * 1024;

/// Read one state file. `None` when it is missing, a symlink or other
/// non-regular file, larger than [`MAX_SESSION_BYTES`], unparsable, or of a
/// different [`SCHEMA`].
#[must_use]
pub fn read_session(path: &Path) -> Option<SessionState> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > MAX_SESSION_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_SESSION_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_SESSION_BYTES {
        return None;
    }
    let state: SessionState = serde_json::from_slice(&bytes).ok()?;
    (state.schema == SCHEMA).then_some(state)
}

/// Every readable state file under `<state_dir>/sessions`, in directory order.
/// Skips dot-files (in-flight temp files), non-`.json` entries, symlinks, and
/// files [`read_session`] rejects. Empty when the directory is missing or is
/// itself a symlink.
#[must_use]
pub fn list_sessions(state_dir: &Path) -> Vec<SessionState> {
    let dir = sessions_dir(state_dir);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_dir()) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && name.ends_with(".json")
        })
        .filter_map(|e| read_session(&e.path()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> SessionState {
        SessionState {
            schema: SCHEMA,
            session_id: id.to_string(),
            host: HostKind::Host,
            hostname: Some("example-host".into()),
            tmux: Some("work:2.1".into()),
            tmux_pane: Some("%7".into()),
            pid: Some(4242),
            cwd: "/work/example".into(),
            repo: Some("/work/example".into()),
            worktree: Some("/work/example".into()),
            branch: Some("main".into()),
            project: Some("demo".into()),
            status: Status::Working,
            started_at: "2026-01-02T03:04:05+09:00".into(),
            updated_at: "2026-01-02T03:05:00+09:00".into(),
        }
    }

    fn write(dir: &Path, name: &str, body: &str) {
        let sessions = sessions_dir(dir);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join(name), body).unwrap();
    }

    #[test]
    fn round_trips_lowercase_enums() {
        let s = sample("abc");
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"status\":\"working\""));
        assert!(json.contains("\"host\":\"host\""));
        assert_eq!(serde_json::from_str::<SessionState>(&json).unwrap(), s);
    }

    #[test]
    fn validates_session_ids() {
        assert!(is_valid_session_id("0b1c2d3e-aaaa-4bbb-8ccc-123456789abc"));
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("../etc"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id("a\tb"));
        assert!(!is_valid_session_id(&"a".repeat(129)));
    }

    #[test]
    fn list_skips_temp_foreign_and_other_schema() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "s1.json",
            &serde_json::to_string(&sample("s1")).unwrap(),
        );
        write(
            tmp.path(),
            ".s2.json.tmp",
            &serde_json::to_string(&sample("s2")).unwrap(),
        );
        write(tmp.path(), "notes.txt", "x");
        write(tmp.path(), "bad.json", "{not json");
        let mut other = sample("s3");
        other.schema = SCHEMA + 1;
        write(
            tmp.path(),
            "s3.json",
            &serde_json::to_string(&other).unwrap(),
        );

        let got = list_sessions(tmp.path());
        assert_eq!(got, vec![sample("s1")]);
    }

    #[test]
    fn oversized_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let mut big = serde_json::to_string(&sample("big")).unwrap();
        big.push_str(&" ".repeat(usize::try_from(MAX_SESSION_BYTES).unwrap()));
        write(tmp.path(), "big.json", &big);
        write(
            tmp.path(),
            "ok.json",
            &serde_json::to_string(&sample("ok")).unwrap(),
        );
        assert_eq!(list_sessions(tmp.path()), vec![sample("ok")]);
    }

    #[test]
    fn list_is_empty_for_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_sessions(&tmp.path().join("nope")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn read_refuses_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside.json");
        std::fs::write(&outside, serde_json::to_string(&sample("x")).unwrap()).unwrap();
        let sessions = sessions_dir(tmp.path());
        std::fs::create_dir_all(&sessions).unwrap();
        std::os::unix::fs::symlink(&outside, sessions.join("x.json")).unwrap();
        assert!(read_session(&sessions.join("x.json")).is_none());
        assert!(list_sessions(tmp.path()).is_empty());
    }
}
