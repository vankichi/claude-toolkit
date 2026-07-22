//! Live aggregation for the "Now" panel — a rollup across every currently
//! active session.
//!
//! Self-contained (does not re-enter ccwatch): each tailed session folds into
//! its own [`Slot`] (tokens, cost, messages, context, model), while tool names
//! and a timestamped token log are kept globally so the recent-tools strip and
//! the real-time tokens/minute rate chart aggregate every session at once.
//! Pure and terminal-free, so it is fully unit-tested by feeding parsed lines.

use ccstat::model::Category;
use cctk::jsonl::{Extracted, Kind, Line, Usage};
use cctk::pricing::{ModelInfo, Pricing};
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, HashMap, VecDeque};

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
    /// Working directory captured from the first line that carries one; its
    /// basename is the session's project name.
    cwd: Option<String>,
    /// Tool-use counts for this session (per-slot so a re-tailed session that
    /// replays from byte 0 rebuilds its counts instead of double-counting).
    tool_counts: HashMap<String, u64>,
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
            cwd: None,
            tool_counts: HashMap::new(),
        }
    }
}

impl Slot {
    fn ingest(&mut self, line: &Line) {
        if self.cwd.is_none()
            && let Some(cwd) = &line.cwd
        {
            self.cwd = Some(cwd.clone());
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
                *self.tool_counts.entry(name.to_string()).or_insert(0) += 1;
            }
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

    /// Project name = last path component of the captured cwd.
    fn project(&self) -> Option<&str> {
        self.cwd
            .as_deref()
            .and_then(|c| c.rsplit(['/', '\\']).find(|s| !s.is_empty()))
    }
}

/// Rollup across all active sessions.
#[derive(Debug, Default)]
pub struct NowStats {
    sessions: BTreeMap<usize, Slot>,
    /// Timestamped usage events across all sessions, oldest first, capped:
    /// `(time, category, name, weight)`. Weight is tokens for `Model` and 1
    /// (one invocation) for agents/skills/commands/MCP, so the Top-usage chart
    /// can plot a real-time rate for any category.
    events: VecDeque<(DateTime<Utc>, Category, String, u64)>,
}

impl NowStats {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one parsed line from session `tag` into the rollup. Non-assistant
    /// lines are ignored.
    pub fn ingest(&mut self, tag: usize, line: &Line) {
        // Timestamped usage events for every category (assistant lines yield a
        // model + any skill/agent/mcp invocations; user lines yield commands).
        if let Some(at) = line.timestamp_utc() {
            for ex in line.extracted() {
                let (category, name, weight) = match ex {
                    Extracted::Model { name, usage } => {
                        let tokens = usage.input_tokens
                            + usage.output_tokens
                            + usage.cache_creation_input_tokens;
                        (Category::Model, name, tokens)
                    }
                    Extracted::Agent { name } => (Category::Agent, name, 1),
                    Extracted::Skill { name } => (Category::Skill, name, 1),
                    Extracted::Command { name } => (Category::Command, name, 1),
                    Extracted::Mcp { server } => (Category::Mcp, server, 1),
                };
                self.events.push_back((at, category, name, weight));
            }
            if self.events.len() > EVENTS_CAP {
                let drop = self.events.len() - EVENTS_CAP;
                self.events.drain(0..drop);
            }
        }

        // Per-session token/cost/context/tool aggregation is assistant-only.
        if line.kind == Kind::Assistant {
            self.sessions.entry(tag).or_default().ingest(line);
        }
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

    /// Distinct project names across active sessions (sorted).
    #[must_use]
    pub fn projects(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .sessions
            .values()
            .filter_map(|s| s.project().map(str::to_string))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
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

    /// Tool-use counts aggregated across all active sessions, most-used first
    /// (ties broken alphabetically).
    #[must_use]
    pub fn tools_by_count(&self) -> Vec<(&str, u64)> {
        let mut agg: HashMap<&str, u64> = HashMap::new();
        for slot in self.sessions.values() {
            for (name, &c) in &slot.tool_counts {
                *agg.entry(name.as_str()).or_insert(0) += c;
            }
        }
        let mut out: Vec<(&str, u64)> = agg.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        out
    }

    /// Real-time usage/minute for `category` (summed across sessions) over the
    /// last `window` seconds, as `bins` `(bin_index, per_minute)` points
    /// spanning `[now - window, now]`. Units are tokens for `Model`, otherwise
    /// invocations. Empty bins read as zero.
    #[must_use]
    pub fn rate_points_total(
        &self,
        category: Category,
        now: DateTime<Utc>,
        window: i64,
        bins: usize,
    ) -> Vec<(f64, f64)> {
        if bins == 0 || window <= 0 {
            return Vec::new();
        }
        let start = now - Duration::seconds(window);
        let bucket_secs = window as f64 / bins as f64;
        let per_minute = 60.0 / bucket_secs;

        let mut buckets = vec![0u64; bins];
        for (ts, cat, _name, weight) in &self.events {
            if *cat != category {
                continue;
            }
            if let Some(idx) = bucket_index(*ts, start, now, bucket_secs, bins) {
                buckets[idx] += weight;
            }
        }
        buckets
            .iter()
            .enumerate()
            .map(|(i, &t)| (i as f64, t as f64 * per_minute))
            .collect()
    }

    /// Per-name real-time usage/minute within `category` over the last `window`
    /// seconds, as `(name, points)` sorted by windowed total descending.
    #[must_use]
    pub fn rate_points_by_name(
        &self,
        category: Category,
        now: DateTime<Utc>,
        window: i64,
        bins: usize,
    ) -> Vec<(String, Vec<(f64, f64)>)> {
        if bins == 0 || window <= 0 {
            return Vec::new();
        }
        let start = now - Duration::seconds(window);
        let bucket_secs = window as f64 / bins as f64;
        let per_minute = 60.0 / bucket_secs;

        // name -> (per-bin weight, windowed total)
        let mut by_name: HashMap<&str, (Vec<u64>, u64)> = HashMap::new();
        for (ts, cat, name, weight) in &self.events {
            if *cat != category {
                continue;
            }
            let Some(idx) = bucket_index(*ts, start, now, bucket_secs, bins) else {
                continue;
            };
            let entry = by_name
                .entry(name.as_str())
                .or_insert_with(|| (vec![0u64; bins], 0));
            entry.0[idx] += weight;
            entry.1 += weight;
        }

        let mut names: Vec<(&str, (Vec<u64>, u64))> = by_name.into_iter().collect();
        names.sort_by(|a, b| b.1.1.cmp(&a.1.1).then_with(|| a.0.cmp(b.0)));
        names
            .into_iter()
            .map(|(name, (buckets, _))| {
                let pts = buckets
                    .iter()
                    .enumerate()
                    .map(|(i, &t)| (i as f64, t as f64 * per_minute))
                    .collect();
                (name.to_string(), pts)
            })
            .collect()
    }
}

/// The bucket index for `ts` within `[start, now]` split into `bins`, or `None`
/// if `ts` is outside the window.
fn bucket_index(
    ts: DateTime<Utc>,
    start: DateTime<Utc>,
    now: DateTime<Utc>,
    bucket_secs: f64,
    bins: usize,
) -> Option<usize> {
    if ts < start || ts > now {
        return None;
    }
    let secs = (ts - start).num_seconds() as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let idx = ((secs / bucket_secs) as usize).min(bins - 1);
    Some(idx)
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
    fn projects_lists_distinct_basenames() {
        use serde_json::json;
        let mut n = NowStats::new();
        let line = |cwd: &str| {
            json!({"type":"assistant","timestamp":"2026-07-22T12:00:00Z","cwd":cwd,
                "message":{"model":"opus","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}).to_string()
        };
        n.ingest(
            0,
            &Line::parse(&line("/Users/me/go/src/github.com/vankichi/claude-toolkit")).unwrap(),
        );
        n.ingest(1, &Line::parse(&line("/Users/me/dotfiles/")).unwrap());
        n.ingest(2, &Line::parse(&line("/Users/me/dotfiles")).unwrap());
        assert_eq!(
            n.projects(),
            vec!["claude-toolkit".to_string(), "dotfiles".to_string()]
        );
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
    fn tools_by_count_aggregates_across_sessions_desc() {
        let mut n = NowStats::new();
        n.ingest(
            0,
            &assistant("sonnet", &["Read", "Bash", "Bash"], (1, 1, 0, 0)),
        );
        n.ingest(1, &assistant("opus", &["Bash", "Edit"], (1, 1, 0, 0)));
        let tools = n.tools_by_count();
        // Bash: 2 + 1 = 3 (most-used, first); Edit/Read tie at 1, alpha order.
        assert_eq!(tools[0], ("Bash", 3));
        assert_eq!(tools[1], ("Edit", 1));
        assert_eq!(tools[2], ("Read", 1));
    }

    #[test]
    fn tool_counts_do_not_double_count_on_reingest_into_fresh_slot() {
        // A dropped + re-tailed session gets a fresh tag/slot, so replaying its
        // lines rebuilds counts instead of doubling them.
        let mut n = NowStats::new();
        n.ingest(0, &assistant("opus", &["Bash", "Bash"], (1, 1, 0, 0)));
        n.drop_session(0);
        n.ingest(1, &assistant("opus", &["Bash", "Bash"], (1, 1, 0, 0)));
        assert_eq!(n.tools_by_count(), vec![("Bash", 2)]);
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
        let pts = n.rate_points_total(Category::Model, now, 120, 2);
        assert_eq!(pts.len(), 2);
        // Both land in bucket 1 (last 60s): 1200 tokens over 60s = 1200 tok/min.
        assert!((pts[1].1 - 1200.0).abs() < 1e-6);
    }

    #[test]
    fn rate_points_by_name_groups_and_sorts_by_total() {
        use chrono::TimeZone;
        let mut n = NowStats::new();
        let now = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();
        n.ingest(
            0,
            &assistant_at(
                "claude-opus-4-8",
                5000,
                &(now - Duration::seconds(30)).to_rfc3339(),
            ),
        );
        n.ingest(
            1,
            &assistant_at(
                "claude-sonnet-5",
                1000,
                &(now - Duration::seconds(40)).to_rfc3339(),
            ),
        );
        // A second opus session in another slot merges into the "opus" line.
        n.ingest(
            2,
            &assistant_at(
                "claude-opus-4-8",
                3000,
                &(now - Duration::seconds(50)).to_rfc3339(),
            ),
        );
        let series = n.rate_points_by_name(Category::Model, now, 120, 2);
        assert_eq!(series.len(), 2);
        // opus (5000 + 3000) sorts before sonnet (1000).
        assert_eq!(series[0].0, "claude-opus-4-8");
        assert_eq!(series[1].0, "claude-sonnet-5");
    }

    #[test]
    fn rate_points_track_agent_and_command_invocations() {
        use chrono::TimeZone;
        use serde_json::json;
        let now = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();
        let ts = (now - Duration::seconds(30)).to_rfc3339();
        let mut n = NowStats::new();
        // An assistant line invoking an Agent, and a user line with a command.
        let agent_line = json!({
            "type": "assistant",
            "timestamp": ts,
            "message": {"model": "opus", "content": [
                {"type": "tool_use", "name": "Agent", "input": {"subagent_type": "code-reviewer"}}
            ], "usage": {"input_tokens": 10, "output_tokens": 5}},
        });
        let cmd_line = json!({
            "type": "user",
            "timestamp": ts,
            "message": {"content": "<command-name>/review</command-name>"},
        });
        n.ingest(0, &Line::parse(&agent_line.to_string()).unwrap());
        n.ingest(0, &Line::parse(&cmd_line.to_string()).unwrap());

        // Agent invocation shows up under the Agent category (1 call).
        let agents = n.rate_points_by_name(Category::Agent, now, 120, 2);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].0, "code-reviewer");
        // Command from the user line is tracked too.
        let cmds = n.rate_points_by_name(Category::Command, now, 120, 2);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].0, "/review");
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
        let pts = n.rate_points_total(Category::Model, now, 900, 15);
        assert!(pts.iter().all(|&(_, y)| y.abs() < f64::EPSILON));
    }
}
