//! Live-activity detection for `--watch` mode.
//!
//! Pure, terminal-free logic: given the tail bytes of a session log and the
//! current time, decide which `(category, name)` pairs are "running now", and
//! pick the spinner frame for a given animation tick. All I/O (finding active
//! files, reading their tails) lives in [`crate::scan`]; everything here is a
//! function of its inputs so it can be unit-tested without a filesystem.
//!
//! "Active" is a heuristic, not a guarantee: an item counts as running when it
//! appears on a log line whose timestamp is no older than `now - window`. A
//! skill/agent/command `tool_use` is an instantaneous event, so this reflects
//! *recent* activity in a live session rather than a still-executing process.

use crate::model::Category;
use cctk::jsonl::{Extracted, Line};
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeSet;

/// The 10-frame Braille spinner shown next to running rows.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The spinner glyph for animation `tick` (wraps around the frame set).
#[must_use]
pub fn spinner_frame(tick: u64) -> char {
    #[allow(clippy::cast_possible_truncation)]
    let idx = (tick % SPINNER.len() as u64) as usize;
    SPINNER[idx]
}

/// The `(category, name)` pairs currently running across live sessions, plus a
/// count of how many sessions are active. Ordered by `(category, name)` so the
/// summary listing is deterministic (`Category`'s `Ord` is tab order).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ActiveSet {
    items: BTreeSet<(Category, String)>,
    session_count: usize,
}

impl ActiveSet {
    /// Fold one file's extracted `(category, name)` pairs into the set.
    pub fn absorb<I: IntoIterator<Item = (Category, String)>>(&mut self, items: I) {
        self.items.extend(items);
    }

    /// Record that one more session is active (mtime within the window),
    /// independent of whether it contributed any in-window items.
    pub fn record_session(&mut self) {
        self.session_count += 1;
    }

    /// Is `(category, name)` currently running? Linear over the (tiny) active
    /// set to avoid allocating an owned key for a `BTreeSet::contains` probe.
    #[must_use]
    pub fn is_active(&self, category: Category, name: &str) -> bool {
        self.items.iter().any(|(c, n)| *c == category && n == name)
    }

    /// Active pairs in `(category, name)` order.
    pub fn iter(&self) -> impl Iterator<Item = &(Category, String)> {
        self.items.iter()
    }

    /// Number of live sessions (mtime within the window).
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.session_count
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The `(category, name)` pairs on lines in `tail` whose timestamp is no older
/// than `now - window`. `tail` must start on a line boundary (the reader
/// guarantees this); a partially-written final line fails to parse and is
/// simply skipped until it is complete on a later poll.
///
/// Lines without a timestamp are ignored for liveness — better to omit a
/// spinner than to mark a stale item that merely happens to sit in the last
/// few KB of a long-running session.
#[must_use]
pub fn active_items_in_tail(
    tail: &[u8],
    now: DateTime<Utc>,
    window: Duration,
) -> Vec<(Category, String)> {
    let cutoff = now - window;
    // Lossy decode keeps every line even across an invalid byte; parse failures
    // collapse to empty and contribute nothing.
    let content = String::from_utf8_lossy(tail);
    let mut out = Vec::new();
    for raw in content.lines() {
        let Some(parsed) = Line::parse(raw) else {
            continue;
        };
        // No timestamp -> can't judge liveness; skip. Future timestamps (clock
        // skew) still count as active (only the lower bound is enforced).
        let Some(ts) = parsed.timestamp_utc() else {
            continue;
        };
        if ts < cutoff {
            continue;
        }
        for item in &parsed.extracted() {
            out.push(item_key(item));
        }
    }
    out
}

/// True if `tail` holds at least one parseable line whose timestamp is within
/// `window` of `now` — i.e. the session emitted a line recently, regardless of
/// whether that line carries an extractable category. Used to mark a live
/// subagent transcript as "running" even when its recent lines are tool
/// results or plain turns.
#[must_use]
pub fn tail_has_recent_line(tail: &[u8], now: DateTime<Utc>, window: Duration) -> bool {
    let cutoff = now - window;
    let content = String::from_utf8_lossy(tail);
    content.lines().any(|raw| {
        Line::parse(raw)
            .and_then(|l| l.timestamp_utc())
            .is_some_and(|ts| ts >= cutoff)
    })
}

/// Map an extracted usage record to its `(category, name)` key.
fn item_key(item: &Extracted) -> (Category, String) {
    match item {
        Extracted::Model { name, .. } => (Category::Model, name.clone()),
        Extracted::Agent { name } => (Category::Agent, name.clone()),
        Extracted::Skill { name } => (Category::Skill, name.clone()),
        Extracted::Command { name } => (Category::Command, name.clone()),
        Extracted::Mcp { server } => (Category::Mcp, server.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 17, 12, 0, 0).unwrap()
    }

    fn window() -> Duration {
        Duration::seconds(90)
    }

    /// An assistant line at `ts` carrying a model plus a skill/agent/mcp `tool_use`.
    fn assistant_line(ts: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"model":"claude-opus-4-8","content":[{{"type":"tool_use","name":"Skill","input":{{"skill":"brainstorm"}}}},{{"type":"tool_use","name":"Agent","input":{{"subagent_type":"go-feature-tdd"}}}},{{"type":"tool_use","name":"mcp__claude_ai_Notion__notion-fetch","input":{{}}}}],"usage":{{"input_tokens":1,"output_tokens":1}}}}}}"#
        )
    }

    #[test]
    fn spinner_wraps_around_frames() {
        assert_eq!(spinner_frame(0), '⠋');
        assert_eq!(spinner_frame(1), '⠙');
        assert_eq!(spinner_frame(10), '⠋'); // 10 frames -> wraps
        assert_eq!(spinner_frame(21), '⠙');
    }

    #[test]
    fn recent_line_yields_all_categories() {
        let tail = assistant_line("2026-07-17T11:59:30Z"); // 30s ago, within 90s
        let items = active_items_in_tail(tail.as_bytes(), now(), window());
        assert!(items.contains(&(Category::Model, "claude-opus-4-8".into())));
        assert!(items.contains(&(Category::Skill, "brainstorm".into())));
        assert!(items.contains(&(Category::Agent, "go-feature-tdd".into())));
        assert!(items.contains(&(Category::Mcp, "claude_ai_Notion".into())));
    }

    #[test]
    fn command_line_is_detected_when_recent() {
        let tail = r#"{"type":"user","timestamp":"2026-07-17T11:59:50Z","message":{"content":"<command-name>/grill-me</command-name>"}}"#;
        let items = active_items_in_tail(tail.as_bytes(), now(), window());
        assert_eq!(items, vec![(Category::Command, "/grill-me".into())]);
    }

    #[test]
    fn line_older_than_window_is_excluded() {
        let tail = assistant_line("2026-07-17T11:00:00Z"); // 1h ago
        let items = active_items_in_tail(tail.as_bytes(), now(), window());
        assert!(items.is_empty());
    }

    #[test]
    fn line_without_timestamp_is_excluded() {
        let tail = r#"{"type":"assistant","message":{"model":"sonnet","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let items = active_items_in_tail(tail.as_bytes(), now(), window());
        assert!(items.is_empty());
    }

    #[test]
    fn future_timestamp_from_clock_skew_still_counts() {
        let tail = assistant_line("2026-07-17T12:00:30Z"); // 30s in the future
        let items = active_items_in_tail(tail.as_bytes(), now(), window());
        assert!(items.contains(&(Category::Model, "claude-opus-4-8".into())));
    }

    #[test]
    fn partial_trailing_line_is_skipped_until_complete() {
        // A recent complete line followed by a half-written JSON line.
        let mut tail = assistant_line("2026-07-17T11:59:30Z");
        tail.push('\n');
        tail.push_str(r#"{"type":"assistant","timestamp":"2026-07-17T11:59:"#);
        let items = active_items_in_tail(tail.as_bytes(), now(), window());
        // Only the first, complete line contributes.
        assert!(items.contains(&(Category::Model, "claude-opus-4-8".into())));
        assert_eq!(
            items.iter().filter(|(c, _)| *c == Category::Model).count(),
            1
        );
    }

    #[test]
    fn tail_has_recent_line_tracks_any_recent_timestamp() {
        // A tool-result user line carries no extractable category, but its
        // recency still marks the (subagent) session as live.
        let recent =
            r#"{"type":"user","timestamp":"2026-07-17T11:59:40Z","message":{"content":"ok"}}"#;
        assert!(tail_has_recent_line(recent.as_bytes(), now(), window()));
        let old =
            r#"{"type":"user","timestamp":"2026-07-17T11:00:00Z","message":{"content":"ok"}}"#;
        assert!(!tail_has_recent_line(old.as_bytes(), now(), window()));
        // No timestamp -> cannot be judged recent.
        let no_ts = r#"{"type":"user","message":{"content":"ok"}}"#;
        assert!(!tail_has_recent_line(no_ts.as_bytes(), now(), window()));
    }

    #[test]
    fn active_set_absorb_dedups_and_orders() {
        let mut set = ActiveSet::default();
        set.absorb(vec![
            (Category::Skill, "b".into()),
            (Category::Model, "opus".into()),
            (Category::Skill, "a".into()),
        ]);
        set.absorb(vec![(Category::Skill, "a".into())]); // dup
        let ordered: Vec<_> = set.iter().cloned().collect();
        assert_eq!(
            ordered,
            vec![
                (Category::Model, "opus".into()),
                (Category::Skill, "a".into()),
                (Category::Skill, "b".into()),
            ]
        );
    }

    #[test]
    fn active_set_is_active_and_session_count() {
        let mut set = ActiveSet::default();
        set.absorb(vec![(Category::Agent, "x".into())]);
        set.record_session();
        set.record_session();
        assert!(set.is_active(Category::Agent, "x"));
        assert!(!set.is_active(Category::Agent, "y"));
        assert!(!set.is_active(Category::Skill, "x"));
        assert_eq!(set.session_count(), 2);
        assert!(!set.is_empty());
        assert!(ActiveSet::default().is_empty());
    }
}
