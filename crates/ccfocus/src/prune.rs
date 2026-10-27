//! Retention for the state directory: event logs older than 90 days, and
//! temp files a killed hook left behind (`.<id>.json.<pid>.<nanos>.tmp`).
//!
//! Run by the dashboards rather than the hook, so prompts never pay for it.
//! Only names the hook itself writes are matched, and only regular files are
//! removed (never through a symlink).

use chrono::{Days, NaiveDate};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Event logs are kept this many days.
pub const KEEP_EVENT_DAYS: u64 = 90;

/// A temp file is renamed within milliseconds of being created; one this old
/// belongs to a hook that died.
pub const TEMP_MAX_AGE: Duration = Duration::from_mins(10);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pruned {
    pub event_logs: usize,
    pub temp_files: usize,
}

/// Remove expired event logs and stale temp files. Errors on single files
/// are skipped (the next run tries again).
#[must_use]
pub fn prune(state_dir: &Path, today: NaiveDate, now: SystemTime) -> Pruned {
    let mut pruned = Pruned::default();
    let oldest_kept = today
        .checked_sub_days(Days::new(KEEP_EVENT_DAYS))
        .unwrap_or(today);
    for (path, name) in regular_files(&cctk::focus::events_dir(state_dir)) {
        let Some(day) = name
            .strip_suffix(".tsv")
            .filter(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y%m%d").ok())
        else {
            continue;
        };
        if day < oldest_kept && std::fs::remove_file(&path).is_ok() {
            pruned.event_logs += 1;
        }
    }
    for (path, name) in regular_files(&cctk::focus::sessions_dir(state_dir)) {
        if !is_hook_temp(&name) {
            continue;
        }
        let old = std::fs::symlink_metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= TEMP_MAX_AGE);
        if old && std::fs::remove_file(&path).is_ok() {
            pruned.temp_files += 1;
        }
    }
    pruned
}

/// `.<session_id>.json.<pid>.<nanos>.tmp`, as `fsx::write_atomic` names them.
fn is_hook_temp(name: &str) -> bool {
    let Some(inner) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else {
        return false;
    };
    let mut parts = inner.rsplitn(3, '.');
    let (Some(nanos), Some(pid), Some(file)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    digits(nanos)
        && digits(pid)
        && file
            .strip_suffix(".json")
            .is_some_and(cctk::focus::is_valid_session_id)
}

/// Regular files directly in `dir` (not followed through symlinks), with
/// their names. Empty when `dir` is missing or not a real directory.
fn regular_files(dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_dir()) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| Some((e.path(), e.file_name().into_string().ok()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "00000000-0000-4000-8000-000000000001";

    #[test]
    fn matches_only_hook_temp_names() {
        assert!(is_hook_temp(&format!(".{SID}.json.4242.123456.tmp")));
        assert!(!is_hook_temp(&format!("{SID}.json")));
        assert!(!is_hook_temp(&format!(".{SID}.lock")));
        assert!(!is_hook_temp(&format!(".{SID}.json.x.1.tmp")));
        assert!(!is_hook_temp(".../x.json.1.2.tmp"));
        assert!(!is_hook_temp(".notes.tmp"));
    }

    #[test]
    fn removes_old_logs_and_stale_temps_only() {
        let tmp = tempfile::tempdir().unwrap();
        let events = cctk::focus::events_dir(tmp.path());
        let sessions = cctk::focus::sessions_dir(tmp.path());
        std::fs::create_dir_all(&events).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        for name in [
            "20251006.tsv",
            "20251007.tsv",
            "20260104.tsv",
            "notes.tsv",
            "1.tsv",
        ] {
            std::fs::write(events.join(name), "").unwrap();
        }
        std::fs::write(sessions.join(format!(".{SID}.json.4242.123.tmp")), "{}").unwrap();
        std::fs::write(sessions.join(format!("{SID}.json")), "{}").unwrap();
        std::fs::write(sessions.join(format!(".{SID}.lock")), "").unwrap();

        let today = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        // Temps are fresh right now: only the log past 90 days goes.
        let fresh = prune(tmp.path(), today, SystemTime::now());
        assert_eq!(
            fresh,
            Pruned {
                event_logs: 1,
                temp_files: 0
            }
        );
        let later = SystemTime::now() + TEMP_MAX_AGE + Duration::from_secs(1);
        assert_eq!(prune(tmp.path(), today, later).temp_files, 1);

        let mut left: Vec<String> = std::fs::read_dir(&events)
            .unwrap()
            .chain(std::fs::read_dir(&sessions).unwrap())
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        let mut want = vec![
            "1.tsv".to_string(),
            "20251007.tsv".into(),
            "20260104.tsv".into(),
            "notes.tsv".into(),
            format!("{SID}.json"),
            format!(".{SID}.lock"),
        ];
        want.sort();
        assert_eq!(left, want);
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("20200101.tsv"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), cctk::focus::events_dir(tmp.path())).unwrap();
        let sessions = cctk::focus::sessions_dir(tmp.path());
        std::fs::create_dir_all(&sessions).unwrap();
        let target = outside.path().join("victim");
        std::fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, sessions.join(format!(".{SID}.json.1.2.tmp"))).unwrap();

        let today = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        let later = SystemTime::now() + TEMP_MAX_AGE * 2;
        assert_eq!(prune(tmp.path(), today, later), Pruned::default());
        assert!(outside.path().join("20200101.tsv").exists());
        assert!(target.exists());
    }
}
