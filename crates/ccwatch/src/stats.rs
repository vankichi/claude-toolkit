use cctk::jsonl::{Kind, Line, Usage};
use cctk::pricing::{ModelInfo, Pricing};
use chrono::{DateTime, Utc};
use ratatui::style::Color;
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

const HISTORY_CAP: usize = 200;
/// Window for rolling cost / throughput rates. Long enough to smooth spikes,
/// short enough to reflect "current pace" rather than session-lifetime average.
const RECENT_WINDOW_SECS: i64 = 600; // 10 minutes
/// Threshold for the active-session indicator. If the latest event is within
/// this many seconds of wall-clock now, mark the session as active.
const ACTIVE_THRESHOLD_SECS: i64 = 120; // 2 minutes

#[derive(Debug, Clone, Copy)]
struct RecentEntry {
    at: DateTime<Utc>,
    cost_usd: f64,
    tokens: u64,
}

/// Aggregate usage stats for one session, plus rolling history for sparkline.
#[derive(Debug)]
pub(crate) struct SessionStats {
    model_raw: Option<String>,
    model_info: ModelInfo,
    pricing: Pricing,
    started_at: Instant,
    total: Usage,
    session_cost_usd: f64,
    last_context_size: u64,
    assistant_messages: u64,
    tool_counts: HashMap<String, u64>,
    /// `output_tokens` per assistant message, oldest first, capped at `HISTORY_CAP`.
    output_history: Vec<u64>,
    /// Working directory captured from the first event that carries one.
    /// Same for every event in a session, so we only set it once.
    cwd: Option<String>,
    /// Sliding window of recent events for cost/min and tokens/min rates.
    /// Pruned to entries within `RECENT_WINDOW_SECS` on each ingest.
    recent: VecDeque<RecentEntry>,
    /// Wall-clock timestamp of the most recently ingested event.
    /// Used to flag the session as active in the header.
    last_event_at: Option<DateTime<Utc>>,
    /// Explicit override from `--context-window` flag. Takes precedence over the model default.
    user_override_context_window: Option<u64>,
    /// Last seen permission mode from `permission-mode` events.
    /// Raw values from JSONL: `default` / `acceptEdits` / `bypassPermissions` / `plan`.
    permission_mode: Option<String>,
}

impl SessionStats {
    /// Construct an empty `SessionStats` with an optional explicit context
    /// window override (typically wired from the `--context-window` flag /
    /// `CCWATCH_CONTEXT_WINDOW` env var).
    pub(crate) fn new(override_context_window: Option<u64>) -> Self {
        let info = ModelInfo::default();
        Self {
            model_raw: None,
            model_info: info,
            pricing: info.pricing(),
            started_at: Instant::now(),
            total: Usage::default(),
            session_cost_usd: 0.0,
            last_context_size: 0,
            assistant_messages: 0,
            tool_counts: HashMap::new(),
            output_history: Vec::new(),
            cwd: None,
            recent: VecDeque::new(),
            last_event_at: None,
            user_override_context_window: override_context_window,
            permission_mode: None,
        }
    }
}

impl Default for SessionStats {
    /// Convenience shorthand for `SessionStats::new(None)` — used by tests
    /// and by reset paths that don't propagate a CLI override.
    fn default() -> Self {
        Self::new(None)
    }
}

impl SessionStats {
    /// Wipe accumulated counts back to zero. Preserves the user-supplied
    /// context window override (still wanted on the new session).
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.user_override_context_window);
    }

    /// Fold one parsed JSONL event into the running aggregates. Non-assistant
    /// events are dropped silently. Updates: model classification (cached on
    /// first occurrence per name), tool counts, token totals, session cost,
    /// last context size, sliding-window history, output token history, and
    /// active-session timestamp.
    pub(crate) fn ingest(&mut self, line: &Line) {
        if line.kind == Kind::PermissionMode {
            if let Some(mode) = &line.permission_mode {
                self.permission_mode = Some(mode.clone());
            }
            return;
        }
        if line.kind != Kind::Assistant {
            return;
        }

        if self.cwd.is_none()
            && let Some(cwd) = &line.cwd
        {
            self.cwd = Some(cwd.clone());
        }

        if let Some(model) = &line.model
            && self.model_raw.as_deref() != Some(model.as_str())
        {
            self.model_raw = Some(model.clone());
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
        self.assistant_messages += 1;
        let event_cost = self.pricing.cost_usd(&usage);
        self.session_cost_usd += event_cost;

        if let Some(at) = line.timestamp_utc() {
            self.last_event_at = Some(at);
            let tokens =
                usage.input_tokens + usage.output_tokens + usage.cache_creation_input_tokens;
            self.recent.push_back(RecentEntry {
                at,
                cost_usd: event_cost,
                tokens,
            });
            self.prune_recent();
        }

        self.output_history.push(usage.output_tokens);
        if self.output_history.len() > HISTORY_CAP {
            let drop = self.output_history.len() - HISTORY_CAP;
            self.output_history.drain(0..drop);
        }
    }

    /// Drop entries older than `RECENT_WINDOW_SECS` from the head of `recent`.
    /// Cutoff is anchored on the latest event we've seen, not wall-clock now,
    /// so replaying old JSONL on startup still yields a meaningful window.
    fn prune_recent(&mut self) {
        let Some(latest) = self.last_event_at else {
            return;
        };
        let cutoff = latest - chrono::Duration::seconds(RECENT_WINDOW_SECS);
        while let Some(front) = self.recent.front() {
            if front.at < cutoff {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }

    // ---- accessors ----

    #[must_use]
    pub(crate) fn model_raw(&self) -> Option<&str> {
        self.model_raw.as_deref()
    }

    /// Full cwd (working directory) recorded in session events.
    #[must_use]
    pub(crate) fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Last path component of `cwd` — typically the project / repo name.
    #[must_use]
    pub(crate) fn project_basename(&self) -> Option<&str> {
        self.cwd
            .as_deref()
            .and_then(|c| c.rsplit(['/', '\\']).find(|s| !s.is_empty()))
    }

    #[must_use]
    pub(crate) fn model_color(&self) -> Color {
        self.model_info.color()
    }

    #[must_use]
    pub(crate) fn totals(&self) -> &Usage {
        &self.total
    }

    #[must_use]
    pub(crate) fn session_cost_usd(&self) -> f64 {
        self.session_cost_usd
    }

    #[must_use]
    pub(crate) fn last_context_size(&self) -> u64 {
        self.last_context_size
    }

    #[must_use]
    pub(crate) fn context_window(&self) -> u64 {
        if let Some(w) = self.user_override_context_window {
            return w;
        }
        self.model_info.context_window()
    }

    #[must_use]
    pub(crate) fn assistant_messages(&self) -> u64 {
        self.assistant_messages
    }

    #[must_use]
    pub(crate) fn output_history(&self) -> &[u64] {
        &self.output_history
    }

    /// Tool call counts sorted by descending count, ties broken alphabetically.
    #[must_use]
    pub(crate) fn tools_sorted(&self) -> Vec<(&str, u64)> {
        let mut items: Vec<(&str, u64)> = self
            .tool_counts
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        items
    }

    #[must_use]
    pub(crate) fn context_pct(&self) -> f64 {
        let win = self.context_window().max(1);
        (self.last_context_size as f64 / win as f64).min(1.0)
    }

    /// `cache_read / (cache_read + input + cache_creation)` — share of context that hit cache.
    #[must_use]
    pub(crate) fn cache_hit_ratio(&self) -> f64 {
        let denom = self.total.input_tokens
            + self.total.cache_creation_input_tokens
            + self.total.cache_read_input_tokens;
        if denom == 0 {
            return 0.0;
        }
        self.total.cache_read_input_tokens as f64 / denom as f64
    }

    /// Time since ccwatch first attached to this session (wall-clock).
    #[must_use]
    pub(crate) fn elapsed_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// Sliding-window throughput in tokens/min over the last `RECENT_WINDOW_SECS`.
    #[must_use]
    pub(crate) fn tokens_per_minute(&self) -> f64 {
        let total: u64 = self.recent.iter().map(|e| e.tokens).sum();
        total as f64 / RECENT_WINDOW_SECS as f64 * 60.0
    }

    /// Sliding-window burn rate in $/hr over the last `RECENT_WINDOW_SECS`.
    /// Returns 0 if no events in window — i.e. session is idle.
    #[must_use]
    pub(crate) fn cost_per_hour(&self) -> f64 {
        let total: f64 = self.recent.iter().map(|e| e.cost_usd).sum();
        total / RECENT_WINDOW_SECS as f64 * 3600.0
    }

    /// True iff the most recent event arrived within `ACTIVE_THRESHOLD_SECS` of now.
    /// Proxy for "this session is currently being used".
    #[must_use]
    pub(crate) fn is_active(&self) -> bool {
        let Some(last) = self.last_event_at else {
            return false;
        };
        let age_secs = (Utc::now() - last).num_seconds();
        (0..ACTIVE_THRESHOLD_SECS).contains(&age_secs)
    }

    /// Wall-clock timestamp of the most recently ingested event, if any.
    /// Exposed for the summary view's "sort by recent activity" comparator.
    #[must_use]
    pub(crate) fn last_event_at(&self) -> Option<DateTime<Utc>> {
        self.last_event_at
    }

    /// Latest permission mode observed (raw JSONL string).
    /// Display-side mapping (e.g. `acceptEdits` → `edit`) happens in `summary`.
    #[must_use]
    pub(crate) fn permission_mode(&self) -> Option<&str> {
        self.permission_mode.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(model: &str, tools: &[&str], usage: (u64, u64, u64, u64)) -> Line {
        assistant_full(model, tools, usage, None, None)
    }

    fn assistant_with_cwd(
        model: &str,
        tools: &[&str],
        usage: (u64, u64, u64, u64),
        cwd: Option<&str>,
    ) -> Line {
        assistant_full(model, tools, usage, cwd, None)
    }

    fn assistant_at(
        model: &str,
        tools: &[&str],
        usage: (u64, u64, u64, u64),
        timestamp: &str,
    ) -> Line {
        assistant_full(model, tools, usage, None, Some(timestamp))
    }

    fn assistant_full(
        model: &str,
        tools: &[&str],
        usage: (u64, u64, u64, u64),
        cwd: Option<&str>,
        timestamp: Option<&str>,
    ) -> Line {
        let (i, o, cw, cr) = usage;
        let content: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| json!({"type": "tool_use", "name": t}))
            .collect();
        let mut obj = json!({
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
        if let Some(c) = cwd {
            obj["cwd"] = json!(c);
        }
        if let Some(t) = timestamp {
            obj["timestamp"] = json!(t);
        }
        Line::parse(&obj.to_string()).expect("valid assistant JSON")
    }

    fn other() -> Line {
        Line::parse(r#"{"type":"file-history-snapshot","messageId":"x"}"#).unwrap()
    }

    fn permission_mode(mode: &str) -> Line {
        Line::parse(&json!({"type": "permission-mode", "permissionMode": mode}).to_string())
            .unwrap()
    }

    #[test]
    fn ingests_assistant_event_and_tracks_tools() {
        let ev = assistant("claude-opus-4-7", &["Bash", "Edit"], (10, 100, 50, 1000));
        let mut s = SessionStats::default();
        s.ingest(&ev);
        assert_eq!(s.totals().output_tokens, 100);
        assert_eq!(s.totals().cache_read_input_tokens, 1000);
        assert_eq!(s.tools_sorted(), vec![("Bash", 1u64), ("Edit", 1u64)]);
        assert_eq!(s.assistant_messages(), 1);
        assert_eq!(s.last_context_size(), 1060);
        assert!(s.session_cost_usd() > 0.0);
        assert_eq!(s.model_color(), Color::Magenta);
    }

    #[test]
    fn ignores_non_assistant() {
        let mut s = SessionStats::default();
        s.ingest(&other());
        assert_eq!(s.assistant_messages(), 0);
    }

    #[test]
    fn accumulates_across_multiple_messages() {
        let mut s = SessionStats::default();
        s.ingest(&assistant("sonnet", &["Bash"], (10, 20, 0, 0)));
        s.ingest(&assistant("sonnet", &["Bash", "Read"], (5, 30, 100, 200)));
        assert_eq!(s.assistant_messages(), 2);
        assert_eq!(s.totals().input_tokens, 15);
        assert_eq!(s.totals().output_tokens, 50);
        assert_eq!(s.totals().cache_creation_input_tokens, 100);
        assert_eq!(s.totals().cache_read_input_tokens, 200);
        let tools = s.tools_sorted();
        assert_eq!(tools[0], ("Bash", 2));
        assert!(tools.contains(&("Read", 1)));
        assert_eq!(s.last_context_size(), 5 + 100 + 200);
    }

    #[test]
    fn cache_hit_ratio_is_zero_with_no_data() {
        let s = SessionStats::default();
        assert!((s.cache_hit_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cache_hit_ratio_calculation() {
        let mut s = SessionStats::default();
        s.ingest(&assistant("sonnet", &[], (100, 50, 0, 900)));
        assert!((s.cache_hit_ratio() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn context_pct_capped_at_one() {
        // Use override to keep window at 100k; observed 300k must cap at 1.0,
        // not auto-promote to 1M.
        let mut s = SessionStats::new(Some(100_000));
        s.ingest(&assistant("sonnet", &[], (300_000, 0, 0, 0)));
        assert!((s.context_pct() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn context_pct_partial() {
        // Default window is 1M → 100k observed = 10%.
        let mut s = SessionStats::default();
        s.ingest(&assistant("sonnet", &[], (50_000, 0, 0, 50_000)));
        assert!((s.context_pct() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn output_history_caps_at_limit() {
        let mut s = SessionStats::default();
        for _ in 0..(HISTORY_CAP + 50) {
            s.ingest(&assistant("sonnet", &[], (1, 1, 0, 0)));
        }
        assert_eq!(s.output_history().len(), HISTORY_CAP);
    }

    #[test]
    fn reset_clears_all_state() {
        let mut s = SessionStats::default();
        s.ingest(&assistant("opus", &["Bash"], (10, 20, 0, 0)));
        assert!(s.assistant_messages() > 0);
        s.reset();
        assert_eq!(s.assistant_messages(), 0);
        assert_eq!(s.totals().output_tokens, 0);
        assert!(s.tools_sorted().is_empty());
        assert!(s.output_history().is_empty());
        assert!(s.model_raw().is_none());
        assert!((s.session_cost_usd() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_tool_use_name_is_skipped() {
        let mut s = SessionStats::default();
        s.ingest(&assistant("sonnet", &[""], (1, 1, 0, 0)));
        assert!(s.tools_sorted().is_empty());
        assert_eq!(s.assistant_messages(), 1);
    }

    #[test]
    fn opus_costs_more_than_sonnet_for_identical_usage() {
        let mut a = SessionStats::default();
        let mut b = SessionStats::default();
        a.ingest(&assistant("claude-opus-4-7", &[], (1000, 1000, 0, 0)));
        b.ingest(&assistant("claude-sonnet-4-6", &[], (1000, 1000, 0, 0)));
        assert!(a.session_cost_usd() > b.session_cost_usd());
    }

    #[test]
    fn override_context_window_takes_precedence() {
        let mut s = SessionStats::new(Some(500_000));
        s.ingest(&assistant("claude-opus-4-7", &[], (10, 10, 0, 0)));
        assert_eq!(s.context_window(), 500_000);
    }

    #[test]
    fn default_context_window_is_1m_for_all_families() {
        let mut s = SessionStats::default();
        s.ingest(&assistant("claude-opus-4-7", &[], (10, 10, 0, 0)));
        assert_eq!(s.context_window(), 1_000_000);
        let mut h = SessionStats::default();
        h.ingest(&assistant("claude-haiku-4-5", &[], (10, 10, 0, 0)));
        assert_eq!(h.context_window(), 1_000_000);
    }

    #[test]
    fn reset_preserves_override() {
        let mut s = SessionStats::new(Some(750_000));
        s.ingest(&assistant("claude-opus-4-7", &[], (210_000, 0, 0, 0)));
        s.reset();
        assert_eq!(s.context_window(), 750_000);
    }

    #[test]
    fn ingests_permission_mode_event() {
        let mut s = SessionStats::default();
        assert_eq!(s.permission_mode(), None);
        s.ingest(&permission_mode("plan"));
        assert_eq!(s.permission_mode(), Some("plan"));
        // Later events overwrite.
        s.ingest(&permission_mode("acceptEdits"));
        assert_eq!(s.permission_mode(), Some("acceptEdits"));
    }

    #[test]
    fn captures_cwd_from_first_event_with_one() {
        let mut s = SessionStats::default();
        // First event has no cwd.
        s.ingest(&assistant_with_cwd("sonnet", &[], (1, 1, 0, 0), None));
        assert_eq!(s.cwd(), None);
        assert_eq!(s.project_basename(), None);
        // Second event provides cwd; we capture it.
        s.ingest(&assistant_with_cwd(
            "sonnet",
            &[],
            (1, 1, 0, 0),
            Some("/home/user/code/alpha-service"),
        ));
        assert_eq!(s.cwd(), Some("/home/user/code/alpha-service"));
        assert_eq!(s.project_basename(), Some("alpha-service"));
    }

    #[test]
    fn cwd_does_not_change_after_first_capture() {
        let mut s = SessionStats::default();
        s.ingest(&assistant_with_cwd(
            "sonnet",
            &[],
            (1, 1, 0, 0),
            Some("/a/b"),
        ));
        s.ingest(&assistant_with_cwd(
            "sonnet",
            &[],
            (1, 1, 0, 0),
            Some("/x/y"),
        ));
        assert_eq!(s.cwd(), Some("/a/b"));
    }

    #[test]
    fn project_basename_handles_trailing_slash() {
        let mut s = SessionStats::default();
        s.ingest(&assistant_with_cwd(
            "sonnet",
            &[],
            (1, 1, 0, 0),
            Some("/foo/bar/"),
        ));
        assert_eq!(s.project_basename(), Some("bar"));
    }

    #[test]
    fn cost_per_hour_uses_only_events_in_window() {
        let mut s = SessionStats::default();
        // Simulate two events: one inside the 10-min window, one outside.
        // Window cutoff is anchored to the latest event.
        s.ingest(&assistant_at(
            "sonnet",
            &[],
            (0, 1_000_000, 0, 0),
            "2026-04-27T12:00:00Z",
        ));
        s.ingest(&assistant_at(
            "sonnet",
            &[],
            (0, 1_000_000, 0, 0),
            "2026-04-27T12:09:00Z",
        ));
        // Both should be in the 10-min window; cost = 2 * sonnet output rate * 1M tokens.
        // = 2 * $15 = $30 over the 600s window → $180/hr.
        let burn = s.cost_per_hour();
        assert!((burn - 180.0).abs() < 0.01, "expected ~$180/hr, got {burn}");
    }

    #[test]
    fn cost_per_hour_drops_old_events_outside_window() {
        let mut s = SessionStats::default();
        s.ingest(&assistant_at(
            "sonnet",
            &[],
            (0, 1_000_000, 0, 0),
            "2026-04-27T12:00:00Z",
        ));
        // 11 minutes later — older event should be pruned from window.
        s.ingest(&assistant_at(
            "sonnet",
            &[],
            (0, 1_000_000, 0, 0),
            "2026-04-27T12:11:00Z",
        ));
        // Only the 2nd event remains in the window: $15 over 600s = $90/hr.
        let burn = s.cost_per_hour();
        assert!((burn - 90.0).abs() < 0.01, "expected ~$90/hr, got {burn}");
    }

    #[test]
    fn cost_per_hour_zero_when_no_events() {
        let s = SessionStats::default();
        assert!((s.cost_per_hour() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tokens_per_minute_uses_sliding_window() {
        let mut s = SessionStats::default();
        s.ingest(&assistant_at(
            "sonnet",
            &[],
            (60, 0, 0, 0),
            "2026-04-27T12:00:00Z",
        ));
        // 60 input tokens over 600s window → 60/600*60 = 6 tok/min
        let tpm = s.tokens_per_minute();
        assert!((tpm - 6.0).abs() < 1e-9, "expected 6 tok/min, got {tpm}");
    }

    #[test]
    fn is_active_false_with_no_events() {
        let s = SessionStats::default();
        assert!(!s.is_active());
    }

    #[test]
    fn is_active_false_for_old_event() {
        let mut s = SessionStats::default();
        // Hard-coded past timestamp: definitely > 2 minutes ago.
        s.ingest(&assistant_at(
            "sonnet",
            &[],
            (1, 1, 0, 0),
            "2020-01-01T00:00:00Z",
        ));
        assert!(!s.is_active());
    }

    #[test]
    fn is_active_true_for_recent_event() {
        let mut s = SessionStats::default();
        // Use a timestamp 10 seconds ago.
        let now = Utc::now() - chrono::Duration::seconds(10);
        let ts = now.to_rfc3339();
        s.ingest(&assistant_at("sonnet", &[], (1, 1, 0, 0), &ts));
        assert!(s.is_active());
    }

    #[test]
    fn tools_sorted_orders_by_count_then_alpha() {
        let mut s = SessionStats::default();
        s.ingest(&assistant(
            "sonnet",
            &["Bash", "Bash", "Edit", "Read"],
            (1, 1, 0, 0),
        ));
        s.ingest(&assistant("sonnet", &["Bash", "Read"], (1, 1, 0, 0)));
        let tools = s.tools_sorted();
        assert_eq!(tools[0], ("Bash", 3));
        assert_eq!(tools[1], ("Read", 2));
        assert_eq!(tools[2], ("Edit", 1));
    }
}
