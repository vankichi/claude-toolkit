//! Enumerate Claude Code session logs and fold them into a `UsageDb`.
//!
//! Files are parsed in parallel with `std::thread::scope` (no async runtime);
//! each worker builds a partial `UsageDb` over a slice of the file list, and
//! the partials are merged. Each session's project label comes from the `cwd`
//! recorded in its events (basename), falling back to the encoded parent
//! directory name.

use crate::jsonl::{self, LineData};
use crate::usage::UsageDb;
use chrono::{DateTime, NaiveDate, Utc};
use std::fs::File;
use std::io::{BufRead, BufReader};
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

/// All `*.jsonl` files exactly one directory below `projects_dir`
/// (`projects_dir/<encoded-project>/<session>.jsonl`).
#[must_use]
pub fn session_files(projects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(project_dirs) = std::fs::read_dir(projects_dir) else {
        return out;
    };
    for pd in project_dirs.flatten() {
        if !pd.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(pd.path()) else {
            continue;
        };
        for se in sessions.flatten() {
            let path = se.path();
            let is_jsonl = path.extension().and_then(|e| e.to_str()) == Some("jsonl");
            let is_file = se.file_type().is_ok_and(|t| t.is_file());
            if is_jsonl && is_file {
                out.push(path);
            }
        }
    }
    out
}

/// Basename of `cwd` if present, else the file's parent directory name.
#[must_use]
pub fn project_label(file: &Path, cwd: Option<&str>) -> String {
    if let Some(cwd) = cwd
        && let Some(base) = cwd.rsplit(['/', '\\']).find(|s| !s.is_empty())
    {
        return base.to_string();
    }
    file.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)")
        .to_string()
}

fn parse_file(path: &Path, today: NaiveDate) -> UsageDb {
    let fallback_day = file_mtime_date(path).unwrap_or(today);
    let Ok(file) = File::open(path) else {
        return UsageDb::default();
    };

    // Buffer this file's parsed lines so we can resolve the project label
    // (which comes from a cwd that may appear on any line) before folding.
    let mut lines: Vec<LineData> = Vec::new();
    let mut cwd: Option<String> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let data = jsonl::parse_line(&line);
        if cwd.is_none()
            && let Some(c) = &data.cwd
        {
            cwd = Some(c.clone());
        }
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

fn file_mtime_date(path: &Path) -> Option<NaiveDate> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let dt: DateTime<Utc> = modified.into();
    Some(dt.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, ProjectFilter, SortKey, Window};
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
    fn session_files_finds_jsonl_one_level_down() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(tmp.path(), "proj-a", "s1.jsonl", &["{}"]);
        write_session(tmp.path(), "proj-a", "s2.jsonl", &["{}"]);
        write_session(tmp.path(), "proj-b", "s3.jsonl", &["{}"]);
        // A non-jsonl file must be ignored.
        write_session(tmp.path(), "proj-b", "notes.txt", &["x"]);
        let mut files = session_files(tmp.path());
        files.sort();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|p| p.extension().unwrap() == "jsonl"));
    }

    #[cfg(unix)]
    #[test]
    fn session_files_excludes_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let real = write_session(tmp.path(), "proj-a", "s1.jsonl", &["{}"]);

        // A symlink named *.jsonl one level down, pointing OUTSIDE the
        // projects tree, must not be collected (read-only contract: only
        // real *.jsonl files under projects_dir may be opened).
        let outside = tempfile::NamedTempFile::new().unwrap();
        let pd = tmp.path().join("proj-a");
        let evil = pd.join("evil.jsonl");
        std::os::unix::fs::symlink(outside.path(), &evil).unwrap();

        let mut files = session_files(tmp.path());
        files.sort();
        assert_eq!(files, vec![real]);
    }

    #[test]
    fn session_files_ignores_top_level_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        // A .jsonl placed directly in projects_dir (zero levels deep) must
        // not be collected; session_files only recurses into directories.
        std::fs::write(tmp.path().join("top-level.jsonl"), "{}").unwrap();
        write_session(tmp.path(), "proj-a", "s1.jsonl", &["{}"]);

        let files = session_files(tmp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap(), "s1.jsonl");
    }

    #[test]
    fn project_label_prefers_cwd_basename() {
        let file = Path::new("/x/-Users-me-repo/session.jsonl");
        assert_eq!(
            project_label(file, Some("/Users/me/work/my-repo")),
            "my-repo"
        );
        assert_eq!(
            project_label(file, Some("/Users/me/work/my-repo/")),
            "my-repo"
        );
    }

    #[test]
    fn project_label_falls_back_to_dir_name() {
        let file = Path::new("/x/-Users-me-repo/session.jsonl");
        assert_eq!(project_label(file, None), "-Users-me-repo");
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
}
