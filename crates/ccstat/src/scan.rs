//! Enumerate Claude Code session logs and fold them into a `UsageDb`.
//!
//! Files are parsed in parallel with `std::thread::scope` (no async runtime);
//! each worker builds a partial `UsageDb` over a slice of the file list, and
//! the partials are merged. Each session's project label comes from the `cwd`
//! recorded in its events (basename), falling back to the encoded parent
//! directory name. Path/tail primitives live in [`cctk::paths`].

use crate::live::{self, ActiveSet};
use crate::model::Category;
use crate::usage::{LineData, UsageDb};
use cctk::jsonl::Line;
use cctk::paths::{is_subagent_file, project_label, read_tail, session_files, subagent_label};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::path::{Path, PathBuf};

pub struct ScanConfig {
    pub projects_dir: PathBuf,
}

/// Scan every session log under `cfg.projects_dir` and return the aggregated
/// usage store.
#[must_use]
pub fn scan(cfg: &ScanConfig, today: NaiveDate) -> UsageDb {
    let files = session_files(&cfg.projects_dir);
    if files.is_empty() {
        return UsageDb::default();
    }

    let workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .min(files.len());

    let chunk_size = files.len().div_ceil(workers);
    let mut total = UsageDb::default();

    std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk_size)
            .map(|chunk| {
                s.spawn(move || {
                    let mut db = UsageDb::default();
                    for file in chunk {
                        db.merge(parse_file(file, today));
                    }
                    db
                })
            })
            .collect();
        for h in handles {
            // A worker panic is a bug (the parse path is panic-free by design); fail
            // loudly rather than silently under-reporting. In release builds
            // (panic = "abort") a worker panic aborts the process before this point.
            total.merge(h.join().expect("ccstat scan worker thread panicked"));
        }
    });

    total
}

fn parse_file(path: &Path, today: NaiveDate) -> UsageDb {
    let fallback_day = file_mtime_date(path).unwrap_or(today);
    let Ok(bytes) = std::fs::read(path) else {
        return UsageDb::default();
    };
    // Lossy UTF-8 decode keeps every line even when a byte is invalid, so one
    // corrupt line can't truncate the rest of the session file.
    let content = String::from_utf8_lossy(&bytes);

    // Buffer this file's parsed lines so we can resolve the project label
    // (which comes from a cwd that may appear on any line) before folding.
    let mut lines: Vec<LineData> = Vec::new();
    let mut cwd: Option<String> = None;
    for raw in content.lines() {
        let Some(parsed) = Line::parse(raw) else {
            continue;
        };
        if cwd.is_none()
            && let Some(c) = &parsed.cwd
        {
            cwd = Some(c.clone());
        }
        let data = LineData::from_line(&parsed);
        if !data.items.is_empty() {
            lines.push(data);
        }
    }

    let project = project_label(path, cwd.as_deref());
    let mut db = UsageDb::default();
    for line in &lines {
        db.absorb(line, &project, fallback_day);
    }
    db
}

fn file_mtime(path: &Path) -> Option<DateTime<Utc>> {
    Some(std::fs::metadata(path).ok()?.modified().ok()?.into())
}

fn file_mtime_date(path: &Path) -> Option<NaiveDate> {
    Some(file_mtime(path)?.date_naive())
}

/// The set of `(category, name)` pairs running "now": for every session whose
/// log was modified within `window` of `now`, read the last `tail_bytes` and
/// collect the items whose line timestamp is within the window. Only active
/// files are read (never the full corpus), so this is cheap enough to poll on
/// a short interval.
#[must_use]
pub fn compute_active(
    cfg: &ScanConfig,
    now: DateTime<Utc>,
    window: Duration,
    tail_bytes: u64,
) -> ActiveSet {
    let mut set = ActiveSet::default();
    for file in session_files(&cfg.projects_dir) {
        let Some(mtime) = file_mtime(&file) else {
            continue;
        };
        // Last write older than the window -> the session is idle. A future
        // mtime (clock skew) yields a negative delta and stays active.
        if now - mtime > window {
            continue;
        }
        set.record_session();
        let tail = read_tail(&file, tail_bytes);
        set.absorb(live::active_items_in_tail(&tail, now, window));
        // A live subagent transcript is itself the signal that an Agent is
        // running: its own lines carry the subagent's model/skills/tools but
        // never an `Agent` tool_use for itself, so surface it by its short id.
        if is_subagent_file(&file) && live::tail_has_recent_line(&tail, now, window) {
            set.absorb(std::iter::once((Category::Agent, subagent_label(&file))));
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, ProjectFilter, SortKey, Window};
    use std::fs::File;
    use std::io::Write;

    fn write_session(dir: &Path, project_dir: &str, file: &str, lines: &[&str]) -> PathBuf {
        let pd = dir.join(project_dir);
        std::fs::create_dir_all(&pd).unwrap();
        let path = pd.join(file);
        let mut f = File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn scan_aggregates_across_files_and_uses_cwd_project() {
        let tmp = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let skill = r#"{"type":"assistant","timestamp":"2026-07-16T10:00:00Z","cwd":"/home/u/alpha","message":{"model":"sonnet","content":[{"type":"tool_use","name":"Skill","input":{"skill":"brainstorm"}}],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        write_session(tmp.path(), "-home-u-alpha", "s1.jsonl", &[skill]);
        write_session(tmp.path(), "-home-u-alpha", "s2.jsonl", &[skill]);

        let db = scan(
            &ScanConfig {
                projects_dir: tmp.path().to_path_buf(),
            },
            today,
        );
        let rows = db.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "brainstorm");
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[0].by_project, vec![("alpha".to_string(), 2)]);
    }

    #[test]
    fn scan_of_empty_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let db = scan(
            &ScanConfig {
                projects_dir: tmp.path().to_path_buf(),
            },
            today,
        );
        assert!(db.is_empty());
    }

    #[test]
    fn scan_of_missing_dir_is_empty() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let db = scan(
            &ScanConfig {
                projects_dir: PathBuf::from("/no/such/dir/xyz"),
            },
            today,
        );
        assert!(db.is_empty());
    }

    #[test]
    fn compute_active_excludes_sessions_stale_by_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let line = r#"{"type":"assistant","timestamp":"2026-07-17T11:59:30Z","message":{"model":"opus","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        write_session(tmp.path(), "p", "s.jsonl", &[line]);
        // `now` far ahead of the file's real mtime -> the session reads as idle.
        let set = compute_active(
            &ScanConfig {
                projects_dir: tmp.path().to_path_buf(),
            },
            Utc::now() + Duration::days(2),
            Duration::seconds(90),
            16 * 1024,
        );
        assert!(set.is_empty());
        assert_eq!(set.session_count(), 0);
    }

    #[test]
    fn compute_active_picks_up_a_live_session() {
        let tmp = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let line = format!(
            r#"{{"type":"assistant","timestamp":"{}","message":{{"model":"opus","content":[{{"type":"tool_use","name":"Skill","input":{{"skill":"brainstorm"}}}}],"usage":{{"input_tokens":1,"output_tokens":1}}}}}}"#,
            now.to_rfc3339()
        );
        write_session(tmp.path(), "p", "s.jsonl", &[&line]);
        let set = compute_active(
            &ScanConfig {
                projects_dir: tmp.path().to_path_buf(),
            },
            now,
            Duration::days(1),
            16 * 1024,
        );
        assert_eq!(set.session_count(), 1);
        assert!(set.is_active(Category::Model, "opus"));
        assert!(set.is_active(Category::Skill, "brainstorm"));
    }

    #[test]
    fn compute_active_surfaces_a_running_subagent_as_an_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let line = format!(
            r#"{{"type":"assistant","timestamp":"{}","isSidechain":true,"message":{{"model":"opus","content":[{{"type":"tool_use","name":"Bash","input":{{"command":"ls"}}}}],"usage":{{"input_tokens":1,"output_tokens":1}}}}}}"#,
            now.to_rfc3339()
        );
        // A live subagent transcript nested under <session>/subagents/.
        let sub = tmp.path().join("p").join("sess").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("agent-aed9405e1964a27e1.jsonl"),
            format!("{line}\n"),
        )
        .unwrap();

        let set = compute_active(
            &ScanConfig {
                projects_dir: tmp.path().to_path_buf(),
            },
            now,
            Duration::days(1),
            16 * 1024,
        );
        // The subagent surfaces as a running Agent by its short handle (the id
        // with its `agent-a` prefix dropped); its Bash tool_use is not a
        // config-map category and is not listed.
        assert!(set.is_active(Category::Agent, "ed9405e1"));
        assert!(set.is_active(Category::Model, "opus"));
        assert_eq!(set.session_count(), 1);
    }
}
