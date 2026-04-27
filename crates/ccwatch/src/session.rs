//! Discovery utilities for Claude Code session JSONL files.
//!
//! Sessions live at `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`.
//! This module enumerates them and derives display-friendly identifiers.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
}
