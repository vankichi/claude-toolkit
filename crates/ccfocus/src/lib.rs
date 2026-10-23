//! ccfocus — hook-driven status board of every Claude Code session.
//!
//! `ccfocus hook <event>` is registered as a Claude Code command hook. It
//! reads the hook's JSON from stdin and records the session's state in
//! `<state>/sessions/<session_id>.json` (schema in [`cctk::focus`]) plus one
//! line in `<state>/events/<YYYYMMDD>.tsv`. Prompt and message bodies are
//! never stored. All writes live in this crate; `cctk` only reads.

pub mod fsx;
pub mod probe;

use anyhow::{Context, Result, bail};
use cctk::focus::{self, SCHEMA, SessionState, Status};
use chrono::{DateTime, FixedOffset, SecondsFormat};
use probe::Probe;
use serde::Deserialize;
use std::path::Path;

/// How much of the transcript tail to scan for a `custom-title` line when the
/// hook input carries no `session_title`.
const TITLE_SCAN_BYTES: u64 = 8 * 1024 * 1024;

/// Longest `project` name kept; longer titles are cut.
const MAX_PROJECT_CHARS: usize = 200;

/// The hook events ccfocus handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    SessionStart,
    UserPromptSubmit,
    Notification,
    Stop,
    SessionEnd,
}

impl Event {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "SessionStart" => Self::SessionStart,
            "UserPromptSubmit" => Self::UserPromptSubmit,
            "Notification" => Self::Notification,
            "Stop" => Self::Stop,
            "SessionEnd" => Self::SessionEnd,
            _ => return None,
        })
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Notification => "Notification",
            Self::Stop => "Stop",
            Self::SessionEnd => "SessionEnd",
        }
    }
}

/// `notification_type`s that mean Claude is blocked on the user.
const WAITING_NOTIFICATIONS: &[&str] = &[
    "permission_prompt",
    "elicitation_dialog",
    "agent_needs_input",
];

/// The fields ccfocus reads from the hook input. Everything else — notably
/// `prompt`, `message`, and `last_assistant_message` — is never deserialized.
#[derive(Debug, Deserialize)]
struct HookInput {
    session_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    session_title: Option<String>,
    #[serde(default)]
    notification_type: Option<String>,
}

/// What [`handle`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The session file was written with this status and an event line appended.
    Written(Status),
    /// `SessionEnd`: the session file was removed (or already absent).
    Removed,
    /// An event that doesn't change state (e.g. an `idle_prompt` notification).
    Ignored,
}

/// Apply one hook event to the state directory.
pub fn handle(
    event_name: &str,
    input: &[u8],
    state_dir: &Path,
    now: DateTime<FixedOffset>,
    probe: &dyn Probe,
) -> Result<Outcome> {
    let Some(event) = Event::parse(event_name) else {
        bail!("unsupported event {event_name:?}");
    };
    let input: HookInput = serde_json::from_slice(input).context("parse hook input")?;
    if !focus::is_valid_session_id(&input.session_id) {
        bail!("invalid session_id");
    }
    let path = focus::session_path(state_dir, &input.session_id);

    let status = match event {
        Event::SessionEnd => {
            // Stay fast (1.5 s budget): no probes, no event line.
            if fsx::is_real_dir(state_dir)? && fsx::is_real_dir(&focus::sessions_dir(state_dir))? {
                fsx::remove_if_present(&path)?;
            }
            return Ok(Outcome::Removed);
        }
        Event::SessionStart => Status::Idle,
        Event::UserPromptSubmit => Status::Working,
        Event::Notification => {
            let ty = input.notification_type.as_deref().unwrap_or_default();
            if !WAITING_NOTIFICATIONS.contains(&ty) {
                return Ok(Outcome::Ignored);
            }
            Status::Waiting
        }
        Event::Stop => Status::Done,
    };

    fsx::ensure_private_dir(state_dir, true)?;
    let sessions = focus::sessions_dir(state_dir);
    let events = focus::events_dir(state_dir);
    fsx::ensure_private_dir(&sessions, false)?;
    fsx::ensure_private_dir(&events, false)?;

    let existing = focus::read_session(&path);
    let stamp = now.to_rfc3339_opts(SecondsFormat::Secs, false);

    // Title precedence: the hook's session_title, then (only on SessionStart
    // or the first sighting, to keep per-prompt hooks cheap) the transcript's
    // last custom-title, then whatever we recorded before.
    let mut project = input.session_title.as_deref().and_then(clean_project);
    if project.is_none() && (event == Event::SessionStart || existing.is_none()) {
        project = input
            .transcript_path
            .as_deref()
            .and_then(|p| last_custom_title(Path::new(p)))
            .as_deref()
            .and_then(clean_project);
    }
    if project.is_none() {
        project = existing.as_ref().and_then(|s| s.project.clone());
    }

    let cwd = input
        .cwd
        .clone()
        .or_else(|| existing.as_ref().map(|s| s.cwd.clone()))
        .unwrap_or_default();
    let git = if cwd.is_empty() {
        probe::GitInfo::default()
    } else {
        probe.git(Path::new(&cwd))
    };
    let tmux = probe.tmux();

    let state = SessionState {
        schema: SCHEMA,
        session_id: input.session_id.clone(),
        host: probe.host(),
        hostname: probe.hostname(),
        tmux: tmux.location,
        tmux_pane: tmux.pane,
        pid: probe.pid(),
        cwd,
        repo: git.repo,
        worktree: git.worktree,
        branch: git.branch,
        project,
        status,
        // Kept across resume (and every later event); new sessions start now.
        started_at: existing.map_or_else(|| stamp.clone(), |s| s.started_at),
        updated_at: stamp.clone(),
    };
    let mut json = serde_json::to_vec_pretty(&state).context("serialize state")?;
    json.push(b'\n');
    fsx::write_atomic(&sessions, &format!("{}.json", input.session_id), &json)?;

    let log = events.join(format!("{}.tsv", now.format("%Y%m%d")));
    fsx::append_line(
        &log,
        &format!("{stamp}\t{}\t{}\n", input.session_id, event.as_str()),
    )?;
    Ok(Outcome::Written(status))
}

/// The last `{"type":"custom-title","customTitle":…}` in the transcript's
/// tail. Refuses symlinks and non-regular files.
fn last_custom_title(transcript: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct TitleLine {
        #[serde(rename = "type")]
        kind: String,
        #[serde(rename = "customTitle")]
        custom_title: Option<String>,
    }
    if !std::fs::symlink_metadata(transcript).is_ok_and(|m| m.file_type().is_file()) {
        return None;
    }
    let tail = cctk::paths::read_tail(transcript, TITLE_SCAN_BYTES);
    tail.split(|&b| b == b'\n')
        .filter(|line| line.windows(14).any(|w| w == b"\"custom-title\""))
        .filter_map(|line| serde_json::from_slice::<TitleLine>(line).ok())
        .filter(|l| l.kind == "custom-title")
        .filter_map(|l| l.custom_title)
        .next_back()
}

/// Strip control characters (they would reach the dashboard's terminal) and
/// cap the length. `None` when nothing printable remains.
fn clean_project(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_PROJECT_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests;
