//! Live aggregation for the "Now" panel — a rollup across every currently
//! active session.
//!
//! Self-contained (does not re-enter ccwatch): each tailed session folds into
//! its own [`SessionTotals`] (tokens, cost, messages, context, model, tools;
//! per session so a re-tailed session that replays from byte 0 rebuilds its
//! counts instead of double-counting), while tool names
//! and a timestamped token log are kept globally so the recent-tools strip and
//! the real-time tokens/minute rate chart aggregate every session at once.
//! Pure and terminal-free, so it is fully unit-tested by feeding parsed lines.

use ccstat::model::Category;
use cctk::jsonl::{Extracted, Kind, Line};
use cctk::session::SessionTotals;
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// Upper bound on retained timestamped events (memory guard; older ones fall
/// outside every rate window anyway).
const EVENTS_CAP: usize = 4096;

/// How full a session's context is, against its model's window.
fn session_context_pct(s: &SessionTotals) -> f64 {
    s.context_pct(s.model_info().context_window())
}

/// Identity of a tailed session: its file-stem id and (once known) the human
/// name from `claude agents --json`.
#[derive(Debug, Default)]
struct SessionMeta {
    id: String,
    name: Option<String>,
}

/// One active session's line for the Now panel.
pub struct SessionView {
    pub project: String,
    pub name: String,
    pub model: String,
    pub tokens: u64,
    pub cost_usd: f64,
    pub context_pct: f64,
}

/// Rollup across all active sessions.
#[derive(Debug, Default)]
pub struct NowStats {
    sessions: BTreeMap<usize, SessionTotals>,
    /// Per-tag session identity (file-stem id + resolved name).
    meta: HashMap<usize, SessionMeta>,
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
                let (category, name) = Category::of(&ex);
                let weight = match &ex {
                    Extracted::Model { usage, .. } => usage.tokens_without_cache_reads(),
                    _ => 1,
                };
                self.events
                    .push_back((at, category, name.to_string(), weight));
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

    /// Register a tailed session's file-stem id (called when a tail spawns).
    pub fn register_session(&mut self, tag: usize, id: String) {
        self.meta
            .entry(tag)
            .or_insert(SessionMeta { id, name: None });
    }

    /// Apply resolved session names (id → name from `claude agents --json`).
    pub fn apply_names(&mut self, names: &HashMap<String, String>) {
        for m in self.meta.values_mut() {
            m.name = names.get(&m.id).cloned();
        }
    }

    /// Forget a session that is no longer active (its totals leave the rollup;
    /// its already-logged events age out of the rate window on their own).
    pub fn drop_session(&mut self, tag: usize) {
        self.sessions.remove(&tag);
        self.meta.remove(&tag);
    }

    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// One line per active session (project · name · model · tokens · cost ·
    /// context), sorted by tokens descending. Name is the resolved agent name,
    /// else the short session id.
    #[must_use]
    pub fn sessions(&self) -> Vec<SessionView> {
        let mut views: Vec<SessionView> = self
            .sessions
            .iter()
            .map(|(tag, slot)| {
                let meta = self.meta.get(tag);
                let name = meta
                    .and_then(|m| m.name.clone())
                    .or_else(|| meta.map(|m| short_id(&m.id)))
                    .unwrap_or_else(|| "?".to_string());
                SessionView {
                    project: slot.project().unwrap_or("?").to_string(),
                    name,
                    model: slot.model().unwrap_or("—").to_string(),
                    tokens: slot.total().total_tokens(),
                    cost_usd: slot.cost_usd(),
                    context_pct: session_context_pct(slot),
                }
            })
            .collect();
        views.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
        views
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
            .filter_map(SessionTotals::model)
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
        self.sessions
            .values()
            .map(|s| s.total().total_tokens())
            .sum()
    }

    #[must_use]
    pub fn cost_usd(&self) -> f64 {
        self.sessions.values().map(SessionTotals::cost_usd).sum()
    }

    #[must_use]
    pub fn assistant_messages(&self) -> u64 {
        self.sessions.values().map(SessionTotals::messages).sum()
    }

    /// Fill fraction of the fullest active session's context window (context is
    /// per-session and can't be summed).
    #[must_use]
    pub fn context_pct(&self) -> f64 {
        self.sessions
            .values()
            .map(session_context_pct)
            .fold(0.0_f64, f64::max)
    }

    /// Tool-use counts aggregated across all active sessions, most-used first
    /// (ties broken alphabetically).
    #[must_use]
    pub fn tools_by_count(&self) -> Vec<(&str, u64)> {
        let mut agg: HashMap<&str, u64> = HashMap::new();
        for slot in self.sessions.values() {
            for (name, &c) in slot.tool_counts() {
                *agg.entry(name.as_str()).or_insert(0) += c;
            }
        }
        let mut out: Vec<(&str, u64)> = agg.into_iter().collect();
        cctk::session::sort_by_count(&mut out);
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
        let Some(w) = RateWindow::new(now, window, bins) else {
            return Vec::new();
        };
        let mut buckets = vec![0u64; bins];
        for (ts, cat, _name, weight) in &self.events {
            if *cat != category {
                continue;
            }
            if let Some(idx) = w.index(*ts) {
                buckets[idx] += weight;
            }
        }
        w.points(&buckets)
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
        let Some(w) = RateWindow::new(now, window, bins) else {
            return Vec::new();
        };
        // name -> (per-bin weight, windowed total)
        let mut by_name: HashMap<&str, (Vec<u64>, u64)> = HashMap::new();
        for (ts, cat, name, weight) in &self.events {
            if *cat != category {
                continue;
            }
            let Some(idx) = w.index(*ts) else {
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
            .map(|(name, (buckets, _))| (name.to_string(), w.points(&buckets)))
            .collect()
    }
}

/// `[now - window, now]` split into `bins` equal buckets.
struct RateWindow {
    start: DateTime<Utc>,
    now: DateTime<Utc>,
    bucket_secs: f64,
    bins: usize,
}

impl RateWindow {
    /// `None` for an empty window or no bins.
    fn new(now: DateTime<Utc>, window: i64, bins: usize) -> Option<Self> {
        (bins > 0 && window > 0).then(|| Self {
            start: now - Duration::seconds(window),
            now,
            bucket_secs: window as f64 / bins as f64,
            bins,
        })
    }

    /// The bucket `ts` falls in, or `None` if it is outside the window.
    fn index(&self, ts: DateTime<Utc>) -> Option<usize> {
        if ts < self.start || ts > self.now {
            return None;
        }
        let secs = (ts - self.start).num_seconds() as f64;
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let idx = ((secs / self.bucket_secs) as usize).min(self.bins - 1);
        Some(idx)
    }

    /// Per-bucket totals as `(bin_index, per_minute)` chart points.
    fn points(&self, buckets: &[u64]) -> Vec<(f64, f64)> {
        let per_minute = 60.0 / self.bucket_secs;
        buckets
            .iter()
            .enumerate()
            .map(|(i, &t)| (i as f64, t as f64 * per_minute))
            .collect()
    }
}

/// Short session id: the first dash-segment of the file-stem uuid.
fn short_id(id: &str) -> String {
    // A subagent transcript stem is `agent-<id>`; label it by the agent name
    // (or short handle) so it reads as a specific child, not a bare "agent".
    if id.starts_with("agent-") {
        return format!("⤷ {}", cctk::paths::subagent_label_from_stem(id));
    }
    id.split('-').next().unwrap_or(id).to_string()
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
        // context is tracked per-session (1060 tokens against opus's window).
        assert!(n.sessions()[0].context_pct > 0.0);
        assert_eq!(n.session_count(), 1);
    }

    #[test]
    fn sessions_view_uses_name_then_short_id_and_sorts_by_tokens() {
        use serde_json::json;
        use std::collections::HashMap;
        let mut n = NowStats::new();
        let line = |cwd: &str, tok: u64| {
            json!({"type":"assistant","timestamp":"2026-07-22T12:00:00Z","cwd":cwd,
                "message":{"model":"opus","content":[],"usage":{"input_tokens":tok,"output_tokens":0}}}).to_string()
        };
        n.register_session(0, "aaaa1111-x-y-z".into());
        n.register_session(1, "bbbb2222-x-y-z".into());
        n.ingest(0, &Line::parse(&line("/w/claude-toolkit", 100)).unwrap());
        n.ingest(1, &Line::parse(&line("/w/dotfiles", 500)).unwrap());
        let mut names = HashMap::new();
        names.insert("aaaa1111-x-y-z".to_string(), "cctop-dev".to_string());
        n.apply_names(&names);
        let v = n.sessions();
        assert_eq!(v.len(), 2);
        // sorted by tokens desc -> dotfiles (500) first
        assert_eq!(v[0].project, "dotfiles");
        assert_eq!(v[0].name, "bbbb2222"); // no name -> short id
        assert_eq!(v[1].project, "claude-toolkit");
        assert_eq!(v[1].name, "cctop-dev"); // resolved name
    }

    #[test]
    fn short_id_marks_subagent_transcripts() {
        // A named subagent shows its name; an anonymous one, its short handle.
        assert_eq!(
            short_id("agent-adev-cycle-run-86696ab82aa59095"),
            "⤷ dev-cycle-run"
        );
        assert_eq!(short_id("agent-aeeea4a16feb464cc"), "⤷ eeea4a16");
        // A normal session uuid keeps its first segment.
        assert_eq!(short_id("83d82d45-9ad1-421e-b175"), "83d82d45");
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
