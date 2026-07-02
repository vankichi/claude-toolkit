//! Discovery utilities for Claude Code session JSONL files.
//!
//! Sessions live at `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`.
//! This module enumerates them and derives display-friendly identifiers.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Enumerate every `*.jsonl` file under `projects_dir/<project>/`, sorted by
/// modification time descending (newest first).
///
/// Returns an empty Vec if `projects_dir` does not exist.
pub(crate) fn list_all_sessions(projects_dir: &Path) -> Result<Vec<PathBuf>> {
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(PathBuf, SystemTime)> = Vec::new();
    for project_entry in std::fs::read_dir(projects_dir)
        .with_context(|| format!("read_dir {}", projects_dir.display()))?
    {
        let project_entry = project_entry?;
        if !project_entry.file_type()?.is_dir() {
            continue;
        }
        for file_entry in std::fs::read_dir(project_entry.path())? {
            let file_entry = file_entry?;
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let mtime = file_entry.metadata()?.modified()?;
            entries.push((path, mtime));
        }
    }
    entries.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));
    Ok(entries.into_iter().map(|(p, _)| p).collect())
}

/// Extract a short session id from a JSONL filename (stem).
#[must_use]
pub(crate) fn short_id(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).map_or_else(
        || "?".to_string(),
        |s| s.split('-').next().unwrap_or(s).to_string(),
    )
}

/// Hard cap on how long we wait for `claude agents --json` before giving up.
/// `claude agents --json` is normally instantaneous (reads local state); the
/// timeout is just a safety net against a hung child stalling the TUI.
const LIVE_AGENTS_TIMEOUT: Duration = Duration::from_secs(5);

/// Status reported by `claude agents --json` for a live agent.
/// Sessions absent from that list map to [`AgentStatus::Offline`] elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentStatusKind {
    Busy,
    Idle,
}

/// Per-session info pulled from `claude agents --json`. Only present when
/// the session is currently running; historic sessions surfaced via
/// `--days` have `agent_info = None`.
#[derive(Debug, Clone)]
pub(crate) struct AgentInfo {
    pub name: Option<String>,
    pub status_kind: AgentStatusKind,
}

/// Raw deserialization target for one entry in `claude agents --json`.
/// All fields are optional/defaulted so an updated CLI schema won't break us.
#[derive(Debug, Deserialize)]
struct AgentEntry {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// One discovered session row: file on disk + (when live) the agent
/// status/name pulled from `claude agents --json`.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredSession {
    pub path: PathBuf,
    /// Truncated id for display (first dash-segment of file stem).
    pub short_id: String,
    /// Full file stem; matches the `sessionId` join key.
    pub session_id: String,
    /// `Some` when the session is currently in `claude agents --json`;
    /// `None` for offline sessions surfaced via `--days` widening.
    pub agent_info: Option<AgentInfo>,
}

/// Query `claude agents --json` and return a map of sessionId → `AgentInfo`.
/// The CLI emits a JSON array of `{ sessionId, name?, status, kind, … }`;
/// we keep the bits we render (name + status) and ignore the rest.
///
/// Errors surface a friendly message for the common cases: `claude` not on
/// PATH, non-zero exit, JSON parse failure, or the timeout firing.
pub(crate) async fn live_agent_info_map() -> Result<HashMap<String, AgentInfo>> {
    let fut = tokio::process::Command::new("claude")
        .args(["agents", "--json"])
        .stdin(std::process::Stdio::null())
        .output();

    let output = match tokio::time::timeout(LIVE_AGENTS_TIMEOUT, fut).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "could not run `claude agents --json`: the `claude` CLI was not found on PATH"
            );
        }
        Ok(Err(e)) => return Err(e).context("spawning `claude agents --json`"),
        Err(_) => anyhow::bail!(
            "`claude agents --json` timed out after {}s",
            LIVE_AGENTS_TIMEOUT.as_secs()
        ),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "`claude agents --json` exited with {}: {}",
            output.status,
            stderr.trim()
        );
    }

    let entries: Vec<AgentEntry> =
        serde_json::from_slice(&output.stdout).context("parsing `claude agents --json` output")?;
    Ok(entries
        .into_iter()
        .map(|e| {
            let status_kind = match e.status.as_deref() {
                Some("busy") => AgentStatusKind::Busy,
                // Treat anything else (idle, missing, unknown future status) as Idle.
                _ => AgentStatusKind::Idle,
            };
            (
                e.session_id,
                AgentInfo {
                    name: e.name.filter(|s| !s.is_empty()),
                    status_kind,
                },
            )
        })
        .collect())
}

/// Build the summary view's session list. Always includes every live agent
/// that has a backing JSONL file; if `days` is `Some(n)`, also unions in
/// historic JSONLs whose mtime falls within the last `n` days.
///
/// Live rows come first; offline rows are appended after. The UI re-sorts
/// the merged list by `last_event_at` once stats have warmed up.
pub(crate) async fn discover_sessions(
    projects_dir: &Path,
    days: Option<u64>,
) -> Result<Vec<DiscoveredSession>> {
    let live = live_agent_info_map().await?;
    let all_paths = list_all_sessions(projects_dir)?;
    let by_id: HashMap<String, PathBuf> = all_paths
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| (s.to_string(), p.clone()))
        })
        .collect();

    let mut out: Vec<DiscoveredSession> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Pass 1: live agents that have a JSONL on disk.
    for (session_id, info) in &live {
        if let Some(path) = by_id.get(session_id) {
            seen.insert(session_id.clone());
            out.push(DiscoveredSession {
                path: path.clone(),
                short_id: short_id(path),
                session_id: session_id.clone(),
                agent_info: Some(info.clone()),
            });
        }
    }

    // Pass 2: historic widening. `list_all_sessions` is already mtime-desc,
    // so we preserve that ordering on the offline rows.
    if let Some(d) = days {
        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(d.saturating_mul(86_400)))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        for path in all_paths {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if seen.contains(stem) {
                continue;
            }
            let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
                continue;
            };
            if mtime < cutoff {
                continue;
            }
            out.push(DiscoveredSession {
                short_id: short_id(&path),
                session_id: stem.to_string(),
                path,
                agent_info: None,
            });
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn short_id_takes_first_dash_segment() {
        let p = PathBuf::from("/tmp/ec65e22c-0dab-4eff-b119-c2e2cb02aa8a.jsonl");
        assert_eq!(short_id(&p), "ec65e22c");
    }

    #[test]
    fn short_id_handles_no_dashes() {
        let p = PathBuf::from("/tmp/abc.jsonl");
        assert_eq!(short_id(&p), "abc");
    }

    #[test]
    fn short_id_returns_question_mark_when_stem_unavailable() {
        assert_eq!(short_id(&PathBuf::from("/")), "?");
    }

    #[test]
    fn list_all_sessions_returns_descending_by_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let proj_a = dir.path().join("a");
        let proj_b = dir.path().join("b");
        fs::create_dir(&proj_a).unwrap();
        fs::create_dir(&proj_b).unwrap();

        let oldest = proj_a.join("old.jsonl");
        let middle = proj_b.join("mid.jsonl");
        let newest = proj_a.join("new.jsonl");
        fs::write(&oldest, b"x").unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(&middle, b"y").unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(&newest, b"z").unwrap();

        let sessions = list_all_sessions(dir.path()).unwrap();
        assert_eq!(sessions, vec![newest, middle, oldest]);
    }

    #[test]
    fn list_all_sessions_returns_empty_for_missing_or_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_all_sessions(dir.path()).unwrap().is_empty());
        assert!(
            list_all_sessions(&PathBuf::from("/no/such/path"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn list_all_sessions_skips_non_jsonl_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("p");
        fs::create_dir(&proj).unwrap();
        fs::write(proj.join("notes.txt"), b"x").unwrap();
        fs::write(proj.join("data.json"), b"x").unwrap();
        assert!(list_all_sessions(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn list_all_sessions_skips_files_at_root() {
        // Files directly under projects_dir (not inside a project subdir) are ignored.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("stray.jsonl"), b"x").unwrap();
        assert!(list_all_sessions(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn agent_entry_parses_status_and_optional_name() {
        let json = r#"[
            {"sessionId":"aaa","status":"busy","name":"my work"},
            {"sessionId":"bbb","status":"idle"},
            {"sessionId":"ccc","status":"weird-future"}
        ]"#;
        let entries: Vec<AgentEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(entries[0].session_id, "aaa");
        assert_eq!(entries[0].name.as_deref(), Some("my work"));
        assert_eq!(entries[0].status.as_deref(), Some("busy"));
        assert!(entries[1].name.is_none());
        assert_eq!(entries[1].status.as_deref(), Some("idle"));
        // Unknown statuses still parse — we'll map them to Idle at the call site.
        assert_eq!(entries[2].status.as_deref(), Some("weird-future"));
    }

    // discover_sessions invokes the `claude` CLI; full coverage lives in the
    // manual smoke test. AgentEntry parsing covers the JSON schema; the rest
    // of the logic (live ∩ JSONL union + --days widening) is a small enough
    // surface that we cover it through end-to-end manual testing.
}
