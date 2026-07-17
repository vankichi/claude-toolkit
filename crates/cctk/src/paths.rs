//! Pure filesystem helpers for locating and reading Claude Code session logs.
//!
//! Sessions live at `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`.
//! Everything here is side-effect-light (read-only, no async) and unit-tested
//! against a temp directory.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// All `*.jsonl` files exactly one directory below `projects_dir`
/// (`projects_dir/<encoded-project>/<session>.jsonl`). Only real files are
/// returned — symlinks and non-`.jsonl` entries are skipped so the read-only
/// contract can't be tricked into following a link outside the tree. Returns
/// an empty Vec when `projects_dir` is missing or unreadable.
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

/// Read the last `max_bytes` of `path`, trimmed to start on a line boundary so
/// callers always see whole lines. Returns the whole file when it is smaller
/// than `max_bytes`, and an empty buffer on any I/O error or when the tail
/// window holds no newline (a single over-long line we can't safely split).
#[must_use]
pub fn read_tail(path: &Path, max_bytes: u64) -> Vec<u8> {
    let Ok(mut f) = File::open(path) else {
        return Vec::new();
    };
    let Ok(meta) = f.metadata() else {
        return Vec::new();
    };
    let len = meta.len();
    if len == 0 {
        return Vec::new();
    }
    if len <= max_bytes {
        // Whole file: byte 0 is already a line boundary.
        let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        return match f.read_to_end(&mut buf) {
            Ok(_) => buf,
            Err(_) => Vec::new(),
        };
    }
    if f.seek(SeekFrom::Start(len - max_bytes)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::with_capacity(usize::try_from(max_bytes).unwrap_or(0));
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    // Drop the (probably partial) first line so we begin on a boundary.
    match buf.iter().position(|&b| b == b'\n') {
        Some(pos) => {
            buf.drain(0..=pos);
            buf
        }
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        // A symlink named *.jsonl pointing OUTSIDE the tree must not be collected.
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
        std::fs::write(tmp.path().join("top-level.jsonl"), "{}").unwrap();
        write_session(tmp.path(), "proj-a", "s1.jsonl", &["{}"]);
        let files = session_files(tmp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap(), "s1.jsonl");
    }

    #[test]
    fn session_files_empty_for_missing_dir() {
        assert!(session_files(&PathBuf::from("/no/such/dir/xyz")).is_empty());
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
    fn read_tail_returns_whole_small_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_session(tmp.path(), "p", "s.jsonl", &["line1", "line2"]);
        assert_eq!(
            String::from_utf8(read_tail(&path, 1024)).unwrap(),
            "line1\nline2\n"
        );
    }

    #[test]
    fn read_tail_starts_on_line_boundary_after_seek() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_session(
            tmp.path(),
            "p",
            "s.jsonl",
            &["aaaaaaaaaa", "bbbbbbbbbb", "cccccccccc"],
        );
        assert_eq!(String::from_utf8(read_tail(&path, 15)).unwrap(), "cccccccccc\n");
    }

    #[test]
    fn read_tail_empty_when_no_newline_in_window() {
        let tmp = tempfile::tempdir().unwrap();
        let pd = tmp.path().join("p");
        std::fs::create_dir_all(&pd).unwrap();
        let path = pd.join("s.jsonl");
        std::fs::write(&path, "x".repeat(100)).unwrap();
        assert!(read_tail(&path, 10).is_empty());
    }
}
