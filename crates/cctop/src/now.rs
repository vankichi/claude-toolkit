//! Live aggregation for the "Now" panel — the active session's running totals.
//!
//! Self-contained (does not re-enter ccwatch): folds `cctk::jsonl::Line`s into
//! token totals, cost, context fill, a recent-tool ring, and a timestamped
//! event log used to derive a real-time tokens/minute rate chart. Pure and
//! terminal-free, so it is fully unit-tested by feeding it parsed lines.

use cctk::jsonl::{Kind, Line, Usage};
use cctk::pricing::{ModelInfo, Pricing};
use chrono::{DateTime, Duration, Utc};
use std::collections::VecDeque;

/// How many recent tool names the bottom band keeps (newest first).
const RECENT_TOOLS_CAP: usize = 32;
/// Upper bound on retained timestamped events (memory guard; older ones fall
/// outside every rate window anyway).
const EVENTS_CAP: usize = 4096;

/// Running aggregate for the active session.
#[derive(Debug)]
pub struct NowStats {
    model: Option<String>,
    model_info: ModelInfo,
    pricing: Pricing,
    total: Usage,
    cost_usd: f64,
    last_context_size: u64,
    assistant_messages: u64,
    /// Newest tool name at the front.
    recent_tools: VecDeque<String>,
    /// `(event_time, tokens)` per assistant message that carried a timestamp,
    /// oldest first, capped. Feeds the real-time rate chart.
    events: VecDeque<(DateTime<Utc>, u64)>,
}

impl Default for NowStats {
    fn default() -> Self {
        let info = ModelInfo::default();
        Self {
            model: None,
            model_info: info,
            pricing: info.pricing(),
            total: Usage::default(),
            cost_usd: 0.0,
            last_context_size: 0,
            assistant_messages: 0,
            recent_tools: VecDeque::new(),
            events: VecDeque::new(),
        }
    }
}

impl NowStats {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one parsed line into the running aggregate. Non-assistant lines are
    /// ignored.
    pub fn ingest(&mut self, line: &Line) {
        if line.kind != Kind::Assistant {
            return;
        }

        if let Some(model) = &line.model
            && self.model.as_deref() != Some(model.as_str())
        {
            self.model = Some(model.clone());
            self.model_info = ModelInfo::parse(model);
            self.pricing = self.model_info.pricing();
        }

        for name in line.tool_use_names() {
            if !name.is_empty() {
                self.recent_tools.push_front(name.to_string());
                self.recent_tools.truncate(RECENT_TOOLS_CAP);
            }
        }

        let Some(usage) = line.usage else { return };
        self.total += usage;
        self.last_context_size = usage.context_size();
        self.cost_usd += self.pricing.cost_usd(&usage);
        self.assistant_messages += 1;

        // Only timestamped messages contribute to the wall-clock rate chart.
        if let Some(at) = line.timestamp_utc() {
            let tokens =
                usage.input_tokens + usage.output_tokens + usage.cache_creation_input_tokens;
            self.events.push_back((at, tokens));
            if self.events.len() > EVENTS_CAP {
                let drop = self.events.len() - EVENTS_CAP;
                self.events.drain(0..drop);
            }
        }
    }

    #[must_use]
    pub fn model_label(&self) -> &str {
        self.model.as_deref().unwrap_or("—")
    }

    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.total.input_tokens
            + self.total.output_tokens
            + self.total.cache_creation_input_tokens
            + self.total.cache_read_input_tokens
    }

    #[must_use]
    pub fn cost_usd(&self) -> f64 {
        self.cost_usd
    }

    #[must_use]
    pub fn assistant_messages(&self) -> u64 {
        self.assistant_messages
    }

    /// Fraction of the model's context window currently in use, capped at 1.0.
    #[must_use]
    pub fn context_pct(&self) -> f64 {
        let window = self.model_info.context_window().max(1);
        (self.last_context_size as f64 / window as f64).min(1.0)
    }

    #[must_use]
    pub fn last_context_size(&self) -> u64 {
        self.last_context_size
    }

    #[must_use]
    pub fn context_window(&self) -> u64 {
        self.model_info.context_window()
    }

    /// Recent tool names, newest first.
    pub fn recent_tools(&self) -> impl Iterator<Item = &str> {
        self.recent_tools.iter().map(String::as_str)
    }

    /// Real-time tokens/minute over the last `window` seconds, as `bins`
    /// `(bin_index, tokens_per_minute)` points spanning `[now - window, now]`.
    /// Empty bins read as zero so the line covers the whole time axis.
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
        n.ingest(&Line::parse(r#"{"type":"user","message":{"content":"hi"}}"#).unwrap());
        assert_eq!(n.assistant_messages(), 0);
        assert_eq!(n.total_tokens(), 0);
    }

    #[test]
    fn accumulates_tokens_cost_and_context() {
        let mut n = NowStats::new();
        n.ingest(&assistant(
            "claude-opus-4-8",
            &["Bash"],
            (10, 100, 50, 1000),
        ));
        assert_eq!(n.model_label(), "claude-opus-4-8");
        assert_eq!(n.total_tokens(), 10 + 100 + 50 + 1000);
        assert!(n.cost_usd() > 0.0);
        // context = input + cache_creation + cache_read = 10 + 50 + 1000
        assert_eq!(n.last_context_size(), 1060);
        assert_eq!(n.assistant_messages(), 1);
    }

    #[test]
    fn context_pct_is_fraction_of_window() {
        let mut n = NowStats::new();
        // 1M window default; 100k in-context -> 0.1
        n.ingest(&assistant("sonnet", &[], (50_000, 0, 0, 50_000)));
        assert!((n.context_pct() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn recent_tools_newest_first_and_capped() {
        let mut n = NowStats::new();
        n.ingest(&assistant("sonnet", &["Read", "Edit"], (1, 1, 0, 0)));
        let tools: Vec<&str> = n.recent_tools().collect();
        // Edit pushed after Read -> Edit is newest (front).
        assert_eq!(tools, vec!["Edit", "Read"]);
    }

    #[test]
    fn empty_tool_names_are_skipped() {
        let mut n = NowStats::new();
        n.ingest(&assistant("sonnet", &[""], (1, 1, 0, 0)));
        assert_eq!(n.recent_tools().count(), 0);
    }

    #[test]
    fn rate_points_bin_tokens_per_minute_over_window() {
        use chrono::TimeZone;
        let mut n = NowStats::new();
        let now = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();
        // Two 600-token messages, one in each 60s bucket of a 120s window.
        n.ingest(&assistant_at(
            "sonnet",
            600,
            &(now - Duration::seconds(90)).to_rfc3339(),
        ));
        n.ingest(&assistant_at(
            "sonnet",
            600,
            &(now - Duration::seconds(30)).to_rfc3339(),
        ));
        let pts = n.rate_points(now, 120, 2);
        assert_eq!(pts.len(), 2);
        // 60s buckets → per-minute factor 1.0 → 600 tok/min in each.
        assert!((pts[0].1 - 600.0).abs() < 1e-6);
        assert!((pts[1].1 - 600.0).abs() < 1e-6);
    }

    #[test]
    fn rate_points_excludes_events_outside_window() {
        use chrono::TimeZone;
        let mut n = NowStats::new();
        let now = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();
        // An event an hour ago is outside a 15-minute window.
        n.ingest(&assistant_at(
            "sonnet",
            1000,
            &(now - Duration::seconds(3600)).to_rfc3339(),
        ));
        let pts = n.rate_points(now, 900, 15);
        assert!(pts.iter().all(|&(_, y)| y.abs() < f64::EPSILON));
    }

    #[test]
    fn opus_costs_more_than_sonnet() {
        let mut a = NowStats::new();
        let mut b = NowStats::new();
        a.ingest(&assistant("claude-opus-4-8", &[], (1000, 1000, 0, 0)));
        b.ingest(&assistant("claude-sonnet-5", &[], (1000, 1000, 0, 0)));
        assert!(a.cost_usd() > b.cost_usd());
    }
}
