//! Live aggregation for the "Now" panel — a rollup across every currently
//! active session.
//!
//! Self-contained (does not re-enter ccwatch): each tailed session folds into
//! its own [`Slot`] (tokens, cost, messages, context, model), while tool names
//! and a timestamped token log are kept globally so the recent-tools strip and
//! the real-time tokens/minute rate chart aggregate every session at once.
//! Pure and terminal-free, so it is fully unit-tested by feeding parsed lines.

use cctk::jsonl::{Kind, Line, Usage};
use cctk::pricing::{ModelInfo, Pricing};
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, VecDeque};

/// How many recent tool names the bottom band keeps (newest first).
const RECENT_TOOLS_CAP: usize = 32;
/// Upper bound on retained timestamped events (memory guard; older ones fall
/// outside every rate window anyway).
const EVENTS_CAP: usize = 4096;

/// Per-session running aggregate.
#[derive(Debug)]
struct Slot {
    model: Option<String>,
    model_info: ModelInfo,
    pricing: Pricing,
    total: Usage,
    cost_usd: f64,
    messages: u64,
    last_context_size: u64,
}

impl Default for Slot {
    fn default() -> Self {
        let info = ModelInfo::default();
        Self {
            model: None,
            model_info: info,
            pricing: info.pricing(),
            total: Usage::default(),
            cost_usd: 0.0,
            messages: 0,
            last_context_size: 0,
        }
    }
}

impl Slot {
    fn ingest(&mut self, line: &Line) {
        if let Some(model) = &line.model
            && self.model.as_deref() != Some(model.as_str())
        {
            self.model = Some(model.clone());
            self.model_info = ModelInfo::parse(model);
            self.pricing = self.model_info.pricing();
        }
        let Some(usage) = line.usage else { return };
        self.total += usage;
        self.last_context_size = usage.context_size();
        self.cost_usd += self.pricing.cost_usd(&usage);
        self.messages += 1;
    }

    fn tokens(&self) -> u64 {
        self.total.input_tokens
            + self.total.output_tokens
            + self.total.cache_creation_input_tokens
            + self.total.cache_read_input_tokens
    }

    fn context_pct(&self) -> f64 {
        let window = self.model_info.context_window().max(1);
        (self.last_context_size as f64 / window as f64).min(1.0)
    }
}

/// Rollup across all active sessions.
#[derive(Debug, Default)]
pub struct NowStats {
    sessions: BTreeMap<usize, Slot>,
    /// Newest tool name at the front, merged across sessions.
    recent_tools: VecDeque<String>,
    /// `(event_time, tokens)` across all sessions, oldest first, capped.
    events: VecDeque<(DateTime<Utc>, u64)>,
}

impl NowStats {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one parsed line from session `tag` into the rollup. Non-assistant
    /// lines are ignored.
    pub fn ingest(&mut self, tag: usize, line: &Line) {
        if line.kind != Kind::Assistant {
            return;
        }

        for name in line.tool_use_names() {
            if !name.is_empty() {
                self.recent_tools.push_front(name.to_string());
                self.recent_tools.truncate(RECENT_TOOLS_CAP);
            }
        }

        if let (Some(usage), Some(at)) = (line.usage, line.timestamp_utc()) {
            let tokens =
                usage.input_tokens + usage.output_tokens + usage.cache_creation_input_tokens;
            self.events.push_back((at, tokens));
            if self.events.len() > EVENTS_CAP {
                let drop = self.events.len() - EVENTS_CAP;
                self.events.drain(0..drop);
            }
        }

        self.sessions.entry(tag).or_default().ingest(line);
    }

    /// Forget a session that is no longer active (its totals leave the rollup;
    /// its already-logged events age out of the rate window on their own).
    pub fn drop_session(&mut self, tag: usize) {
        self.sessions.remove(&tag);
    }

    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Header label: the model for a single session, or `N sessions · models…`.
    #[must_use]
    pub fn model_label(&self) -> String {
        let mut models: Vec<&str> = self
            .sessions
            .values()
            .filter_map(|s| s.model.as_deref())
            .collect();
        models.sort_unstable();
        models.dedup();
        match self.sessions.len() {
            0 => "—".to_string(),
            1 => models.first().copied().unwrap_or("—").to_string(),
            n if models.is_empty() => format!("{n} sessions"),
            n => format!("{n} sessions · {}", models.join(", ")),
        }
    }

    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.sessions.values().map(Slot::tokens).sum()
    }

    #[must_use]
    pub fn cost_usd(&self) -> f64 {
        self.sessions.values().map(|s| s.cost_usd).sum()
    }

    #[must_use]
    pub fn assistant_messages(&self) -> u64 {
        self.sessions.values().map(|s| s.messages).sum()
    }

    /// Fill fraction of the fullest active session's context window (context is
    /// per-session and can't be summed).
    #[must_use]
    pub fn context_pct(&self) -> f64 {
        self.sessions
            .values()
            .map(Slot::context_pct)
            .fold(0.0_f64, f64::max)
    }

    /// Largest last-seen context size across active sessions.
    #[must_use]
    pub fn last_context_size(&self) -> u64 {
        self.sessions
            .values()
            .map(|s| s.last_context_size)
            .max()
            .unwrap_or(0)
    }

    #[must_use]
    pub fn context_window(&self) -> u64 {
        self.sessions
            .values()
            .next()
            .map_or(1_000_000, |s| s.model_info.context_window())
    }

    /// Recent tool names, newest first, across all sessions.
    pub fn recent_tools(&self) -> impl Iterator<Item = &str> {
        self.recent_tools.iter().map(String::as_str)
    }

    /// Real-time tokens/minute (summed across sessions) over the last `window`
    /// seconds, as `bins` `(bin_index, tokens_per_minute)` points spanning
    /// `[now - window, now]`. Empty bins read as zero.
    #[must_use]
    pub fn rate_points(&self, now: DateTime<Utc>, window: i64, bins: usize) -> Vec<(f64, f64)> {
        if bins == 0 || window <= 0 {
            return Vec::new();
        }
        let start = now - Duration::seconds(window);
        let bucket_secs = window as f64 / bins as f64;
        let per_minute = 60.0 / bucket_secs;

        let mut buckets = vec![0u64; bins];
        for &(ts, tokens) in &self.events {
            if ts < start || ts > now {
                continue;
            }
            let secs = (ts - start).num_seconds() as f64;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let idx = ((secs / bucket_secs) as usize).min(bins - 1);
            buckets[idx] += tokens;
        }
        buckets
            .iter()
            .enumerate()
            .map(|(i, &t)| (i as f64, t as f64 * per_minute))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(model: &str, tools: &[&str], usage: (u64, u64, u64, u64)) -> Line {
        let (i, o, cw, cr) = usage;
        let content: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| json!({"type": "tool_use", "name": t}))
            .collect();
        let obj = json!({
            "type": "assistant",
            "message": {
                "model": model,
                "content": content,
                "usage": {
                    "input_tokens": i,
                    "output_tokens": o,
                    "cache_creation_input_tokens": cw,
                    "cache_read_input_tokens": cr,
                },
            },
        });
        Line::parse(&obj.to_string()).unwrap()
    }

    fn assistant_at(model: &str, tokens: u64, ts: &str) -> Line {
        let obj = json!({
            "type": "assistant",
            "timestamp": ts,
            "message": {
                "model": model,
                "content": [],
                "usage": {
                    "input_tokens": tokens,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                },
            },
        });
        Line::parse(&obj.to_string()).unwrap()
    }

    #[test]
    fn ignores_non_assistant() {
        let mut n = NowStats::new();
        n.ingest(
            0,
            &Line::parse(r#"{"type":"user","message":{"content":"hi"}}"#).unwrap(),
        );
        assert_eq!(n.assistant_messages(), 0);
        assert_eq!(n.total_tokens(), 0);
        assert_eq!(n.session_count(), 0);
    }

    #[test]
    fn single_session_accumulates() {
        let mut n = NowStats::new();
        n.ingest(
            0,
            &assistant("claude-opus-4-8", &["Bash"], (10, 100, 50, 1000)),
        );
        assert_eq!(n.model_label(), "claude-opus-4-8");
        assert_eq!(n.total_tokens(), 10 + 100 + 50 + 1000);
        assert!(n.cost_usd() > 0.0);
        assert_eq!(n.last_context_size(), 1060);
        assert_eq!(n.session_count(), 1);
    }

    #[test]
    fn sums_tokens_cost_messages_across_sessions() {
        let mut n = NowStats::new();
        n.ingest(0, &assistant("claude-opus-4-8", &[], (100, 100, 0, 0)));
        n.ingest(1, &assistant("claude-sonnet-5", &[], (200, 200, 0, 0)));
        assert_eq!(n.session_count(), 2);
        assert_eq!(n.total_tokens(), 600);
        assert_eq!(n.assistant_messages(), 2);
        // Cost is per-line (each session's own model pricing), then summed.
        let opus_only = {
            let mut m = NowStats::new();
            m.ingest(0, &assistant("claude-opus-4-8", &[], (100, 100, 0, 0)));
            m.cost_usd()
        };
        assert!(n.cost_usd() > opus_only);
    }

    #[test]
    fn model_label_reports_session_count_when_multiple() {
        let mut n = NowStats::new();
        n.ingest(0, &assistant("claude-opus-4-8", &[], (1, 1, 0, 0)));
        n.ingest(1, &assistant("claude-sonnet-5", &[], (1, 1, 0, 0)));
        let label = n.model_label();
        assert!(label.starts_with("2 sessions"), "got {label}");
        assert!(label.contains("claude-opus-4-8") && label.contains("claude-sonnet-5"));
    }

    #[test]
    fn context_pct_is_the_fullest_session() {
        let mut n = NowStats::new();
        // 1M window; session 0 at 100k (10%), session 1 at 300k (30%).
        n.ingest(0, &assistant("sonnet", &[], (100_000, 0, 0, 0)));
        n.ingest(1, &assistant("sonnet", &[], (300_000, 0, 0, 0)));
        assert!((n.context_pct() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn drop_session_removes_its_totals() {
        let mut n = NowStats::new();
        n.ingest(0, &assistant("opus", &[], (100, 0, 0, 0)));
        n.ingest(1, &assistant("sonnet", &[], (200, 0, 0, 0)));
        assert_eq!(n.total_tokens(), 300);
        n.drop_session(1);
        assert_eq!(n.session_count(), 1);
        assert_eq!(n.total_tokens(), 100);
    }

    #[test]
    fn recent_tools_merge_newest_first() {
        let mut n = NowStats::new();
        n.ingest(0, &assistant("sonnet", &["Read"], (1, 1, 0, 0)));
        n.ingest(1, &assistant("opus", &["Edit"], (1, 1, 0, 0)));
        let tools: Vec<&str> = n.recent_tools().collect();
        assert_eq!(tools, vec!["Edit", "Read"]);
    }

    #[test]
    fn rate_points_sum_across_sessions_over_window() {
        use chrono::TimeZone;
        let mut n = NowStats::new();
        let now = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();
        // Two sessions each emit 600 tokens in the same 60s bucket.
        n.ingest(
            0,
            &assistant_at("opus", 600, &(now - Duration::seconds(30)).to_rfc3339()),
        );
        n.ingest(
            1,
            &assistant_at("sonnet", 600, &(now - Duration::seconds(20)).to_rfc3339()),
        );
        let pts = n.rate_points(now, 120, 2);
        assert_eq!(pts.len(), 2);
        // Both land in bucket 1 (last 60s): 1200 tokens over 60s = 1200 tok/min.
        assert!((pts[1].1 - 1200.0).abs() < 1e-6);
    }

    #[test]
    fn rate_points_excludes_events_outside_window() {
        use chrono::TimeZone;
        let mut n = NowStats::new();
        let now = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();
        n.ingest(
            0,
            &assistant_at(
                "sonnet",
                1000,
                &(now - Duration::seconds(3600)).to_rfc3339(),
            ),
        );
        let pts = n.rate_points(now, 900, 15);
        assert!(pts.iter().all(|&(_, y)| y.abs() < f64::EPSILON));
    }
}
