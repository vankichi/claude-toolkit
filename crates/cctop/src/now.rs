//! Live aggregation for the "Now" panel — the active session's running totals.
//!
//! Self-contained (does not re-enter ccwatch): folds `cctk::jsonl::Line`s into
//! token totals, cost, context fill, a recent-tool ring, and a token-rate
//! series for the sparkline. Pure and terminal-free, so it is fully unit-
//! tested by feeding it parsed lines.

use cctk::jsonl::{Kind, Line, Usage};
use cctk::pricing::{ModelInfo, Pricing};
use std::collections::VecDeque;

/// How many recent tool names the bottom band keeps (newest first).
const RECENT_TOOLS_CAP: usize = 32;
/// How many per-message token samples the rate sparkline keeps.
const RATE_SERIES_CAP: usize = 120;

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
    /// Per-assistant-message total tokens, oldest first, capped.
    rate_series: Vec<f64>,
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
            rate_series: Vec::new(),
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

        let tokens = usage.input_tokens + usage.output_tokens + usage.cache_creation_input_tokens;
        self.rate_series.push(tokens as f64);
        if self.rate_series.len() > RATE_SERIES_CAP {
            let drop = self.rate_series.len() - RATE_SERIES_CAP;
            self.rate_series.drain(0..drop);
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

    #[must_use]
    pub fn rate_series(&self) -> &[f64] {
        &self.rate_series
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
    fn rate_series_grows_per_message() {
        let mut n = NowStats::new();
        n.ingest(&assistant("sonnet", &[], (10, 20, 0, 0)));
        n.ingest(&assistant("sonnet", &[], (5, 5, 0, 0)));
        assert_eq!(n.rate_series().len(), 2);
        assert!((n.rate_series()[0] - 30.0).abs() < f64::EPSILON);
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
