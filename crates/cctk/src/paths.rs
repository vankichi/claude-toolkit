//! Pure filesystem helpers for locating and reading Claude Code session logs.
//!
//! Sessions live at `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`.
//! Everything here is side-effect-light (read-only, no async) and unit-tested
//! against a temp directory.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// All session-transcript `*.jsonl` files under `projects_dir`: the per-session
/// files one level down (`projects_dir/<encoded-project>/<session>.jsonl`) plus
/// subagent transcripts nested deeper (`…/<session>/subagents/agent-<id>.jsonl`,
/// written while a Task/Agent subagent runs). Recurses into real subdirectories
/// only — symlinked files and directories are never collected or followed, so
/// the read-only contract can't be tricked into leaving the tree — and is
/// bounded to [`MAX_SESSION_DEPTH`] as a loop guard. Returns an empty Vec when
/// `projects_dir` is missing or unreadable.
#[must_use]
pub fn session_files(projects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(project_dirs) = std::fs::read_dir(projects_dir) else {
        return out;
    };
    for pd in project_dirs.flatten() {
        // Descend only into real project directories — never a `*.jsonl` sitting
        // directly in `projects_dir`, nor a symlink — preserving the original
        // "ignore top-level files, don't follow links" contract.
        if pd.file_type().is_ok_and(|t| t.is_dir()) {
            collect_jsonl(&pd.path(), 1, &mut out);
        }
    }
    out
}

/// Depth limit for [`session_files`]' descent below `projects_dir`. Subagent
/// transcripts sit two levels below a project dir (`<session>/subagents/…`); the
/// headroom tolerates future nesting while capping pathologically deep trees.
const MAX_SESSION_DEPTH: usize = 8;

/// Collect real `*.jsonl` files in `dir`, recursing into real subdirectories up
/// to [`MAX_SESSION_DEPTH`]. `depth` is `dir`'s level below `projects_dir`
/// (1 = a project dir). Symlinks are neither collected nor followed.
fn collect_jsonl(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let path = e.path();
        if ft.is_file() {
            if path.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                out.push(path);
            }
        } else if ft.is_dir() && depth < MAX_SESSION_DEPTH {
            collect_jsonl(&path, depth + 1, out);
        }
        // Symlinks (ft.is_symlink()): neither collected nor followed.
    }
}

/// True when `file` is a subagent transcript — it lives under a `subagents/`
/// directory (`…/<session>/subagents/agent-<id>.jsonl`) rather than being a
/// top-level per-session transcript.
#[must_use]
pub fn is_subagent_file(file: &Path) -> bool {
    file.components().any(|c| c.as_os_str() == "subagents")
}

/// Display label for a subagent transcript, telling *which* agent is running.
///
/// The runtime ids a subagent `a[<name>-]<16-hex>` and stores its transcript as
/// `agent-<id>.jsonl`. A *named* agent keeps its name in the id
/// (`agent-adev-cycle-run-86696ab82aa59095` → `dev-cycle-run`); an anonymous one
/// is just `agent-a<16-hex>`, for which we fall back to the short handle
/// (`agent-aeeea4a16feb464cc` → `eeea4a16`). The agent *type* (general-purpose,
/// …) is not recorded in the transcript while it runs, so anonymous agents can
/// only be shown by handle. Falls back to `agent` when the stem is unreadable.
#[must_use]
pub fn subagent_label(file: &Path) -> String {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("agent");
    subagent_label_from_stem(stem)
}

/// [`subagent_label`] from a file *stem* (`agent-<id>`), for callers that hold
/// the stem string rather than a path.
#[must_use]
pub fn subagent_label_from_stem(stem: &str) -> String {
    let id = stem.strip_prefix("agent-").unwrap_or(stem);
    // Drop the single leading 'a' the runtime prefixes onto every agent id.
    let body = id.strip_prefix('a').unwrap_or(id);
    // A named agent is `<name>-<16-hex>`; the tail is the handle. If there is a
    // name before it, that name is the most useful label.
    if let Some((name, handle)) = body.rsplit_once('-')
        && is_hex16(handle)
        && !name.is_empty()
    {
        return name.to_string();
    }
    // Anonymous / unrecognised: the short handle.
    let handle = body.rsplit('-').next().unwrap_or(body);
    if handle.is_empty() {
        "agent".to_string()
    } else {
        handle.chars().take(8).collect()
    }
}

/// True for a 16-char lowercase-hex agent handle.
fn is_hex16(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())
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

    #[test]
    fn session_files_includes_subagent_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        // A normal per-session transcript...
        write_session(tmp.path(), "proj-a", "sess.jsonl", &["{}"]);
        // ...plus a subagent transcript nested under <session>/subagents/.
        let sub = tmp.path().join("proj-a").join("sess").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        let agent = sub.join("agent-aed9405e1964a27e1.jsonl");
        std::fs::write(&agent, "{}\n").unwrap();

        let files = session_files(tmp.path());
        assert!(
            files.contains(&agent),
            "subagent transcript must be discovered"
        );
        assert!(files.iter().any(|p| p.file_name().unwrap() == "sess.jsonl"));
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn is_subagent_file_detects_subagents_dir() {
        assert!(is_subagent_file(Path::new(
            "/x/-proj/sess/subagents/agent-abc123.jsonl"
        )));
        assert!(!is_subagent_file(Path::new("/x/-proj/sess.jsonl")));
    }

    #[test]
    fn subagent_label_extracts_name_or_short_handle() {
        // Anonymous agent (`a<16-hex>`) -> short handle.
        assert_eq!(
            subagent_label(Path::new("/x/subagents/agent-aeeea4a16feb464cc.jsonl")),
            "eeea4a16"
        );
        // Named agent (`a<name>-<16-hex>`) -> the name.
        assert_eq!(
            subagent_label(Path::new(
                "/x/subagents/agent-adev-cycle-run-86696ab82aa59095.jsonl"
            )),
            "dev-cycle-run"
        );
        // A multi-segment name is kept whole.
        assert_eq!(
            subagent_label_from_stem("agent-adev-cycle-wkb113-5bced311577d9e38"),
            "dev-cycle-wkb113"
        );
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
        assert_eq!(
            String::from_utf8(read_tail(&path, 15)).unwrap(),
            "cccccccccc\n"
        );
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
