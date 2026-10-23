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
use std::io::Read;
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
    /// After a batch of tool calls resolves: ends a `waiting` state once the
    /// user has answered the permission prompt.
    PostToolBatch,
    Stop,
    /// Runs instead of `Stop` when the turn ends on an API error.
    StopFailure,
    SessionEnd,
}

impl Event {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "SessionStart" => Self::SessionStart,
            "UserPromptSubmit" => Self::UserPromptSubmit,
            "Notification" => Self::Notification,
            "PostToolBatch" => Self::PostToolBatch,
            "Stop" => Self::Stop,
            "StopFailure" => Self::StopFailure,
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
            Self::PostToolBatch => "PostToolBatch",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::SessionEnd => "SessionEnd",
        }
    }
}

/// `notification_type`s that mean Claude is blocked on the user.
const WAITING_NOTIFICATIONS: &[&str] = &[
    "permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

/// The fields ccfocus reads from the hook input. Everything else — notably
/// `prompt`, `message`, `last_assistant_message`, `tool_calls`, and
/// `error_details` — is never deserialized.
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
    /// `SessionStart` only: startup / resume / clear / compact / fork.
    #[serde(default)]
    source: Option<String>,
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
        // Refined below: a mid-turn compaction keeps the current status.
        Event::SessionStart => Status::Idle,
        Event::UserPromptSubmit => Status::Working,
        Event::Notification => {
            let ty = input.notification_type.as_deref().unwrap_or_default();
            if !WAITING_NOTIFICATIONS.contains(&ty) {
                return Ok(Outcome::Ignored);
            }
            Status::Waiting
        }
        Event::PostToolBatch => return resume_after_wait(state_dir, &path, now),
        Event::Stop => Status::Done,
        Event::StopFailure => Status::Failed,
    };

    // The root may be a directory the user pointed us at, so only the
    // subdirectories we own are tightened when found wider than 0700.
    fsx::ensure_private_dir(state_dir, fsx::Create::WithParents)?;
    fsx::ensure_private_dir(&focus::sessions_dir(state_dir), fsx::Create::Owned)?;
    fsx::ensure_private_dir(&focus::events_dir(state_dir), fsx::Create::Owned)?;

    let existing = focus::read_session(&path);
    let stamp = rfc3339(&now);
    let transcript = input.transcript_path.as_deref().map(Path::new);

    // Auto-compaction fires SessionStart in the middle of a turn; it must not
    // flip a working (or waiting) session back to idle.
    let status = match (&existing, input.source.as_deref()) {
        (Some(prev), Some("compact")) if event == Event::SessionStart => prev.status,
        _ => status,
    };
    let project = resolve_project(&input, event, existing.as_ref(), transcript);

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
    // The hostname can't change within a session; skip the probe once known.
    let hostname = existing
        .as_ref()
        .and_then(|s| s.hostname.clone())
        .or_else(|| probe.hostname());
    // Kept while the file lives. SessionEnd deletes it, so a resumed session
    // recovers its original start from the transcript's first timestamp.
    let started_at = match existing {
        Some(prev) => prev.started_at,
        None if event == Event::SessionStart && input.source.as_deref() == Some("resume") => {
            transcript.and_then(first_timestamp).map_or_else(
                || stamp.clone(),
                |t| rfc3339(&t.with_timezone(&now.timezone())),
            )
        }
        None => stamp.clone(),
    };

    let state = SessionState {
        schema: SCHEMA,
        session_id: input.session_id.clone(),
        host: probe.host(),
        hostname,
        tmux: tmux.location,
        tmux_pane: tmux.pane,
        pid: probe.pid(),
        cwd,
        repo: git.repo,
        worktree: git.worktree,
        branch: git.branch,
        project,
        status,
        started_at,
        updated_at: stamp,
    };
    persist(state_dir, &state, event, now)?;
    Ok(Outcome::Written(status))
}

/// `PostToolBatch` fires after every batch of tool calls, so it stays cheap: no
/// probes, no directories created, and nothing written unless a waiting
/// session has had its prompt answered (the approved tools just ran).
fn resume_after_wait(state_dir: &Path, path: &Path, now: DateTime<FixedOffset>) -> Result<Outcome> {
    if !(fsx::is_real_dir(state_dir)? && fsx::is_real_dir(&focus::sessions_dir(state_dir))?) {
        return Ok(Outcome::Ignored);
    }
    let Some(mut state) = focus::read_session(path) else {
        return Ok(Outcome::Ignored);
    };
    if state.status != Status::Waiting {
        return Ok(Outcome::Ignored);
    }
    state.status = Status::Working;
    state.updated_at = rfc3339(&now);
    fsx::ensure_private_dir(&focus::events_dir(state_dir), fsx::Create::Owned)?;
    persist(state_dir, &state, Event::PostToolBatch, now)?;
    Ok(Outcome::Written(Status::Working))
}

/// Write `state` atomically and log `event`. Hooks of one session can overlap
/// (a slow git probe in `UserPromptSubmit` while a `Notification` lands); if a
/// hook that started after `now` has already written, its newer state is kept
/// and only the event line is added.
fn persist(
    state_dir: &Path,
    state: &SessionState,
    event: Event,
    now: DateTime<FixedOffset>,
) -> Result<()> {
    let sessions = focus::sessions_dir(state_dir);
    if !newer_on_disk(
        &sessions.join(focus::session_file_name(&state.session_id)),
        now,
    ) {
        let mut json = serde_json::to_vec_pretty(state).context("serialize state")?;
        json.push(b'\n');
        fsx::write_atomic(
            &sessions,
            &focus::session_file_name(&state.session_id),
            &json,
        )?;
    }
    let log = focus::events_dir(state_dir).join(format!("{}.tsv", now.format("%Y%m%d")));
    fsx::append_line(
        &log,
        &format!(
            "{}\t{}\t{}\n",
            rfc3339(&now),
            state.session_id,
            event.as_str()
        ),
    )
}

/// Title precedence: the hook's `session_title`, then (only on `SessionStart`
/// or the first sighting, to keep per-prompt hooks cheap) the transcript's
/// last custom-title, then whatever was recorded before.
fn resolve_project(
    input: &HookInput,
    event: Event,
    existing: Option<&SessionState>,
    transcript: Option<&Path>,
) -> Option<String> {
    input
        .session_title
        .as_deref()
        .and_then(clean_project)
        .or_else(|| {
            (event == Event::SessionStart || existing.is_none())
                .then(|| transcript.and_then(last_custom_title))
                .flatten()
                .as_deref()
                .and_then(clean_project)
        })
        .or_else(|| existing.and_then(|s| s.project.clone()))
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
    // Cheap pre-filter so only candidate lines are parsed as JSON.
    const NEEDLE: &[u8] = b"\"custom-title\"";
    if !std::fs::symlink_metadata(transcript).is_ok_and(|m| m.file_type().is_file()) {
        return None;
    }
    let tail = cctk::paths::read_tail(transcript, TITLE_SCAN_BYTES);
    tail.split(|&b| b == b'\n')
        .filter(|line| line.windows(NEEDLE.len()).any(|w| w == NEEDLE))
        .filter_map(|line| serde_json::from_slice::<TitleLine>(line).ok())
        .filter(|l| l.kind == "custom-title")
        .filter_map(|l| l.custom_title)
        .next_back()
}

/// Timestamps are written with millisecond precision so overlapping hooks of
/// one session can be ordered (see [`newer_on_disk`]).
fn rfc3339<Tz: chrono::TimeZone>(t: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    t.to_rfc3339_opts(SecondsFormat::Millis, false)
}

/// True when the session file on disk was written by a hook that started
/// after `now`.
fn newer_on_disk(path: &Path, now: DateTime<FixedOffset>) -> bool {
    focus::read_session(path)
        .and_then(|s| DateTime::parse_from_rfc3339(&s.updated_at).ok())
        .is_some_and(|t| t > now)
}

/// How much of the transcript head to scan for its first `timestamp`.
const START_SCAN_BYTES: u64 = 64 * 1024;

/// The first `timestamp` in the transcript: when the session was created.
/// Refuses symlinks and non-regular files.
fn first_timestamp(transcript: &Path) -> Option<DateTime<FixedOffset>> {
    #[derive(Deserialize)]
    struct Stamped {
        timestamp: Option<String>,
    }
    if !std::fs::symlink_metadata(transcript).is_ok_and(|m| m.file_type().is_file()) {
        return None;
    }
    let mut head = Vec::new();
    std::fs::File::open(transcript)
        .ok()?
        .take(START_SCAN_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    head.split(|&b| b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Stamped>(line).ok())
        .find_map(|l| DateTime::parse_from_rfc3339(l.timestamp.as_deref()?).ok())
}

/// Strip control characters and invisible formatting (bidi overrides,
/// zero-width marks) — they would reach the dashboard's terminal and could
/// reorder or hide text — and cap the length. `None` when nothing printable
/// remains.
fn clean_project(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|&c| !c.is_control() && !is_invisible_format(c))
        .take(MAX_PROJECT_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Unicode format characters (category Cf) that change how surrounding text
/// is shown: soft hyphen, Arabic letter mark, zero-width and directional
/// marks, bidi embeddings/overrides/isolates, invisible operators, BOM.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests;
