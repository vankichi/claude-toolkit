//! Numbers from the event log `<state>/events/<YYYYMMDD>.tsv`
//! (`<RFC 3339>\t<session_id>\t<Event>` per line).
//!
//! Each day file is folded into a [`DayStats`] as it is read and the lines are
//! dropped, so a dashboard left open for days holds a few numbers per day, not
//! the log. Days that no longer change are read once; today's file is read
//! again only when its size changes.

use cctk::focus;
use chrono::{DateTime, Days, FixedOffset, NaiveDate, Timelike};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Width of the window "parallel" counts sessions in: the dashboard's
/// header uses the last one, the day stats use the aligned ones.
pub const WINDOW_SECS: i64 = 30 * 60;

/// Day files larger than this are only read up to it.
const MAX_DAY_BYTES: u64 = 64 * 1024 * 1024;

/// Days kept in memory: today, yesterday, and the rest of the week chart.
const KEEP_DAYS: u64 = 7;

/// One day's figures.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayStats {
    /// `UserPromptSubmit` events.
    pub prompts: u32,
    /// Prompts sent to a different session than the previous prompt that day.
    pub switches: u32,
    /// Mean, over the day's 30-minute windows that saw any event, of the
    /// sessions with an event in the window.
    pub parallel_avg: f64,
    /// Most sessions seen in one of those windows.
    pub parallel_max: usize,
}

/// One parsed line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line<'a> {
    pub at: DateTime<FixedOffset>,
    pub session_id: &'a str,
    pub event: &'a str,
}

/// `None` for lines that aren't three tab-separated fields with an RFC 3339
/// time and a valid session id.
#[must_use]
pub fn parse_line(line: &str) -> Option<Line<'_>> {
    let mut fields = line.trim_end_matches('\r').split('\t');
    let at = DateTime::parse_from_rfc3339(fields.next()?).ok()?;
    let session_id = fields.next()?;
    let event = fields.next()?;
    if fields.next().is_some() || !focus::is_valid_session_id(session_id) {
        return None;
    }
    Some(Line {
        at,
        session_id,
        event,
    })
}

/// Running fold of one day's lines. Today's fold is kept so each refresh
/// only feeds the bytes appended since the last one.
#[derive(Debug, Default)]
struct Fold {
    prompts: u32,
    switches: u32,
    previous_prompt: Option<String>,
    /// Sessions seen in each 30-minute window of the day (at most 48 windows).
    windows: BTreeMap<u32, HashSet<String>>,
    last_seen: HashMap<String, DateTime<FixedOffset>>,
}

impl Fold {
    fn feed(&mut self, text: &str) {
        // Lines are appended as hooks run, so file order is time order.
        for line in text.lines().filter_map(parse_line) {
            let window = line.at.num_seconds_from_midnight() / 1800;
            let seen = self.windows.entry(window).or_default();
            if !seen.contains(line.session_id) {
                seen.insert(line.session_id.to_string());
            }
            self.last_seen
                .entry(line.session_id.to_string())
                .and_modify(|t| *t = (*t).max(line.at))
                .or_insert(line.at);
            if line.event == "UserPromptSubmit" {
                self.prompts += 1;
                if self
                    .previous_prompt
                    .as_deref()
                    .is_some_and(|p| p != line.session_id)
                {
                    self.switches += 1;
                }
                if self.previous_prompt.as_deref() != Some(line.session_id) {
                    self.previous_prompt = Some(line.session_id.to_string());
                }
            }
        }
    }

    fn stats(&self) -> DayStats {
        let mut stats = DayStats {
            prompts: self.prompts,
            switches: self.switches,
            ..DayStats::default()
        };
        if !self.windows.is_empty() {
            let total: usize = self.windows.values().map(HashSet::len).sum();
            stats.parallel_avg = total as f64 / self.windows.len() as f64;
            stats.parallel_max = self.windows.values().map(HashSet::len).max().unwrap_or(0);
        }
        stats
    }
}

/// Fold one day file's text. Also returns each session's last event time, for
/// [`EventLog::active_since`].
#[must_use]
pub fn fold_day(text: &str) -> (DayStats, HashMap<String, DateTime<FixedOffset>>) {
    let mut fold = Fold::default();
    fold.feed(text);
    (fold.stats(), fold.last_seen)
}

/// Read side of the event log with per-day caching.
#[derive(Debug)]
pub struct EventLog {
    dir: PathBuf,
    days: BTreeMap<NaiveDate, Cached>,
}

#[derive(Debug)]
struct Cached {
    /// Bytes of the file folded so far (whole lines only).
    read: u64,
    stats: DayStats,
    /// Today's running fold; `None` for days that no longer grow.
    fold: Option<Fold>,
    /// Kept for today and yesterday only (a 30-minute window can reach back
    /// across midnight); dropped for older days.
    last_seen: HashMap<String, DateTime<FixedOffset>>,
}

impl EventLog {
    #[must_use]
    pub fn new(state_dir: &Path) -> Self {
        Self {
            dir: focus::events_dir(state_dir),
            days: BTreeMap::new(),
        }
    }

    /// Bring the cached week ending `today` up to date with the files.
    pub fn refresh(&mut self, today: NaiveDate) {
        let first = today
            .checked_sub_days(Days::new(KEEP_DAYS))
            .unwrap_or(today);
        let yesterday = today.pred_opt().unwrap_or(today);
        self.days.retain(|d, _| (first..=today).contains(d));
        for day in first.iter_days().take_while(|d| *d <= today) {
            let path = self.dir.join(format!("{}.tsv", day.format("%Y%m%d")));
            let Some(len) = std::fs::symlink_metadata(&path)
                .ok()
                .filter(|m| m.file_type().is_file())
                .map(|m| m.len())
            else {
                self.days.remove(&day);
                continue;
            };
            let cached = self.days.get_mut(&day);
            if cached.as_ref().is_some_and(|c| c.read == len) {
                continue;
            }
            // Today grows by appends: fold only what was added. A file that
            // shrank (or an older day that changed) is folded again in full.
            let (mut fold, from) = match cached {
                Some(c) if day == today && c.fold.is_some() && len > c.read => {
                    (c.fold.take().unwrap_or_default(), c.read)
                }
                _ => (Fold::default(), 0),
            };
            let Some(chunk) = read_lines_from(&path, from) else {
                continue;
            };
            fold.feed(&chunk);
            let read = from + chunk.len() as u64;
            let stats = fold.stats();
            let (fold, last_seen) = if day == today {
                let last_seen = fold.last_seen.clone();
                (Some(fold), last_seen)
            } else if day == yesterday {
                (None, fold.last_seen)
            } else {
                (None, HashMap::new())
            };
            self.days.insert(
                day,
                Cached {
                    read,
                    stats,
                    fold,
                    last_seen,
                },
            );
        }
        for (day, cached) in &mut self.days {
            if *day < today {
                cached.fold = None;
            }
            if *day < yesterday && !cached.last_seen.is_empty() {
                cached.last_seen = HashMap::new();
            }
        }
    }

    /// The figures for `day` (zero when it has no file).
    #[must_use]
    pub fn day(&self, day: NaiveDate) -> DayStats {
        self.days
            .get(&day)
            .map(|c| c.stats.clone())
            .unwrap_or_default()
    }

    /// Distinct sessions with an event at or after `cutoff` (within the
    /// cached today and yesterday).
    #[must_use]
    pub fn active_since(&self, cutoff: DateTime<FixedOffset>) -> usize {
        self.days
            .values()
            .flat_map(|c| c.last_seen.iter())
            .filter(|(_, t)| **t >= cutoff)
            .map(|(id, _)| id.as_str())
            .collect::<HashSet<_>>()
            .len()
    }
}

/// The complete lines of `path` from byte `from` on (a line still being
/// appended is left for the next read), at most [`MAX_DAY_BYTES`].
fn read_lines_from(path: &Path, from: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_DAY_BYTES.saturating_sub(from))
        .read_to_end(&mut bytes)
        .ok()?;
    let end = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    bytes.truncate(end);
    // Lines are written whole (one O_APPEND write each), so a cut never
    // splits a character; lossy only guards a corrupt file.
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "00000000-0000-4000-8000-00000000000a";
    const B: &str = "00000000-0000-4000-8000-00000000000b";
    const C: &str = "00000000-0000-4000-8000-00000000000c";

    fn line(time: &str, sid: &str, event: &str) -> String {
        format!("2026-01-05T{time}.000+09:00\t{sid}\t{event}\n")
    }

    #[test]
    fn parse_rejects_malformed_lines() {
        assert!(parse_line(&line("10:00:00", A, "Stop")).is_some());
        assert!(parse_line("not a time\tx\tStop").is_none());
        assert!(parse_line("2026-01-05T10:00:00+09:00\t../x\tStop").is_none());
        assert!(parse_line("2026-01-05T10:00:00+09:00\tabc").is_none());
        assert!(parse_line("2026-01-05T10:00:00+09:00\tabc\tStop\textra").is_none());
    }

    #[test]
    fn counts_switches_between_sessions() {
        let text = [
            line("09:00:00", A, "SessionStart"),
            line("09:00:05", A, "UserPromptSubmit"),
            line("09:01:00", A, "UserPromptSubmit"),
            line("09:02:00", B, "UserPromptSubmit"),
            line("09:02:30", A, "Stop"),
            line("09:03:00", A, "UserPromptSubmit"),
            line("09:04:00", A, "UserPromptSubmit"),
            "garbage\n".to_string(),
        ]
        .concat();
        let (stats, last_seen) = fold_day(&text);
        assert_eq!(stats.prompts, 5);
        // A → B, B → A; repeated prompts to A and non-prompt events don't count.
        assert_eq!(stats.switches, 2);
        assert_eq!(last_seen.len(), 2);
    }

    #[test]
    fn parallel_is_sessions_per_active_half_hour() {
        let text = [
            // 09:00–09:30: A, B, C
            line("09:00:00", A, "UserPromptSubmit"),
            line("09:10:00", B, "UserPromptSubmit"),
            line("09:29:59", C, "Stop"),
            // 09:30–10:00: A
            line("09:45:00", A, "Stop"),
            // 10:00–11:00: nothing (not averaged in)
            // 11:00–11:30: A, B
            line("11:05:00", A, "UserPromptSubmit"),
            line("11:06:00", B, "Notification"),
        ]
        .concat();
        let (stats, _) = fold_day(&text);
        assert_eq!(stats.parallel_max, 3);
        assert!((stats.parallel_avg - 2.0).abs() < 1e-9);
    }

    #[test]
    fn empty_day_is_zero() {
        assert_eq!(fold_day("").0, DayStats::default());
    }

    fn write_day(dir: &Path, day: &str, text: &str) {
        let events = focus::events_dir(dir);
        std::fs::create_dir_all(&events).unwrap();
        std::fs::write(events.join(format!("{day}.tsv")), text).unwrap();
    }

    #[test]
    fn log_reads_the_week_and_rereads_a_grown_file() {
        let tmp = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        write_day(
            tmp.path(),
            "20260104",
            &format!(
                "2026-01-04T23:50:00.000+09:00\t{A}\tUserPromptSubmit\n2026-01-04T23:51:00.000+09:00\t{B}\tUserPromptSubmit\n"
            ),
        );
        write_day(tmp.path(), "20251220", &line("09:00:00", C, "Stop"));
        write_day(
            tmp.path(),
            "20260105",
            &line("00:05:00", C, "UserPromptSubmit"),
        );

        let mut log = EventLog::new(tmp.path());
        log.refresh(today);
        assert_eq!(log.day(today.pred_opt().unwrap()).switches, 1);
        assert_eq!(log.day(today).prompts, 1);
        // Outside the cached week.
        assert_eq!(
            log.day(NaiveDate::from_ymd_opt(2025, 12, 20).unwrap()),
            DayStats::default()
        );
        // The last 30 minutes reach back into yesterday.
        let now = DateTime::parse_from_rfc3339("2026-01-05T00:10:00+09:00").unwrap();
        assert_eq!(log.active_since(now - chrono::TimeDelta::minutes(30)), 3);
        assert_eq!(log.active_since(now - chrono::TimeDelta::minutes(10)), 1);

        let mut text = line("00:05:00", C, "UserPromptSubmit");
        text.push_str(&line("00:06:00", A, "UserPromptSubmit"));
        write_day(tmp.path(), "20260105", &text);
        log.refresh(today);
        assert_eq!(log.day(today).prompts, 2);
        assert_eq!(log.day(today).switches, 1);
    }

    #[test]
    fn a_half_written_line_waits_for_its_newline() {
        let tmp = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        let full = line("09:00:00", A, "UserPromptSubmit");
        let next = line("09:01:00", B, "UserPromptSubmit");
        write_day(tmp.path(), "20260105", &format!("{full}{}", &next[..20]));
        let mut log = EventLog::new(tmp.path());
        log.refresh(today);
        assert_eq!(log.day(today).prompts, 1);
        write_day(tmp.path(), "20260105", &format!("{full}{next}"));
        log.refresh(today);
        assert_eq!((log.day(today).prompts, log.day(today).switches), (2, 1));
        // Replaced by a shorter file: folded again from the start.
        write_day(tmp.path(), "20260105", &next);
        log.refresh(today);
        assert_eq!((log.day(today).prompts, log.day(today).switches), (1, 0));
    }

    #[test]
    fn old_days_drop_their_session_ids() {
        let tmp = tempfile::tempdir().unwrap();
        write_day(
            tmp.path(),
            "20260101",
            &format!("2026-01-01T10:00:00.000+09:00\t{A}\tUserPromptSubmit\n"),
        );
        let mut log = EventLog::new(tmp.path());
        log.refresh(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap());
        assert_eq!(
            log.days.values().map(|c| c.last_seen.len()).sum::<usize>(),
            1
        );
        log.refresh(NaiveDate::from_ymd_opt(2026, 1, 5).unwrap());
        assert_eq!(
            log.days.values().map(|c| c.last_seen.len()).sum::<usize>(),
            0
        );
        assert_eq!(
            log.day(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())
                .prompts,
            1
        );
    }
}
