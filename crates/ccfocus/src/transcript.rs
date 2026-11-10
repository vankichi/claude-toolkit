//! Read-only peeks into a session's transcript, for the facts the hook
//! input doesn't carry. Symlinks and non-regular files are never read.

use crate::fsx;
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use std::io::Read;
use std::path::Path;

/// How much of the transcript tail to scan for a `custom-title` line when the
/// hook input carries no `session_title`.
const TITLE_SCAN_BYTES: u64 = 8 * 1024 * 1024;

/// The last `{"type":"custom-title","customTitle":…}` in the transcript's
/// tail. Refuses symlinks and non-regular files.
pub(crate) fn last_custom_title(transcript: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct TitleLine {
        #[serde(rename = "type")]
        kind: String,
        #[serde(rename = "customTitle")]
        custom_title: Option<String>,
    }
    // Cheap pre-filter so only candidate lines are parsed as JSON.
    const NEEDLE: &[u8] = b"\"custom-title\"";
    if !fsx::is_regular_file(transcript) {
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

/// How much of the transcript head to scan for its first `timestamp`.
const START_SCAN_BYTES: u64 = 64 * 1024;

/// The first `timestamp` in the transcript: when the session was created.
/// Refuses symlinks and non-regular files.
pub(crate) fn first_timestamp(transcript: &Path) -> Option<DateTime<FixedOffset>> {
    #[derive(Deserialize)]
    struct Stamped {
        timestamp: Option<String>,
    }
    if !fsx::is_regular_file(transcript) {
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
