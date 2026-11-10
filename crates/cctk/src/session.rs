//! Running totals for one session's assistant lines, shared by the live
//! views (`ccwatch`'s per-session stats and `cctop`'s Now rollup) so both
//! count model, cost, tokens, context and tool calls the same way.

use crate::jsonl::{Line, Usage};
use crate::pricing::ModelInfo;
use std::collections::HashMap;

/// What one session has used so far.
#[derive(Debug, Clone, Default)]
pub struct SessionTotals {
    model: Option<String>,
    model_info: ModelInfo,
    total: Usage,
    cost_usd: f64,
    messages: u64,
    last_context_size: u64,
    /// Working directory from the first line that carries one (the same for
    /// every line of a session).
    cwd: Option<String>,
    tool_counts: HashMap<String, u64>,
}

impl SessionTotals {
    /// Fold one assistant line in (the caller picks which lines count).
    /// Returns the line's usage and its cost when it carried usage, for
    /// callers that keep their own per-event history.
    pub fn ingest(&mut self, line: &Line) -> Option<(Usage, f64)> {
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
        }
        for name in line.tool_use_names() {
            if !name.is_empty() {
                *self.tool_counts.entry(name.to_string()).or_insert(0) += 1;
            }
        }
        let usage = line.usage?;
        let cost = self.model_info.pricing().cost_usd(&usage);
        self.total += usage;
        self.last_context_size = usage.context_size();
        self.cost_usd += cost;
        self.messages += 1;
        Some((usage, cost))
    }

    /// The raw model name of the latest line that named one.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    #[must_use]
    pub fn model_info(&self) -> ModelInfo {
        self.model_info
    }

    #[must_use]
    pub fn total(&self) -> &Usage {
        &self.total
    }

    #[must_use]
    pub fn cost_usd(&self) -> f64 {
        self.cost_usd
    }

    /// Assistant lines that carried usage.
    #[must_use]
    pub fn messages(&self) -> u64 {
        self.messages
    }

    /// Context sent on the latest line with usage.
    #[must_use]
    pub fn last_context_size(&self) -> u64 {
        self.last_context_size
    }

    #[must_use]
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// The project name: the cwd's last component.
    #[must_use]
    pub fn project(&self) -> Option<&str> {
        self.cwd.as_deref().and_then(crate::paths::cwd_basename)
    }

    #[must_use]
    pub fn tool_counts(&self) -> &HashMap<String, u64> {
        &self.tool_counts
    }

    /// How full a `window`-token context is, 0.0 to 1.0.
    #[must_use]
    pub fn context_pct(&self, window: u64) -> f64 {
        (self.last_context_size as f64 / window.max(1) as f64).min(1.0)
    }
}

/// Sort `(name, count)` pairs most-counted first, ties by name ascending —
/// the order every tool-count and project-count list is shown in.
pub fn sort_by_count<K: Ord>(items: &mut [(K, u64)]) {
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_by_count_puts_the_most_first_then_names_in_order() {
        let mut items = vec![("b", 1), ("c", 3), ("a", 1)];
        sort_by_count(&mut items);
        assert_eq!(items, [("c", 3), ("a", 1), ("b", 1)]);
    }

    fn line(json: &str) -> Line {
        Line::parse(json).unwrap()
    }

    #[test]
    fn folds_model_usage_cost_cwd_and_tools() {
        let mut s = SessionTotals::default();
        let first = line(
            r#"{"type":"assistant","cwd":"/work/repo","message":{"model":"claude-opus-4-7","content":[{"type":"tool_use","name":"Bash","input":{}}],"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":1000}}}"#,
        );
        let (usage, cost) = s.ingest(&first).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert!(cost > 0.0);
        assert_eq!(s.model(), Some("claude-opus-4-7"));
        assert_eq!(s.project(), Some("repo"));
        assert_eq!(s.last_context_size(), 1100);
        assert_eq!(s.messages(), 1);
        assert_eq!(s.tool_counts().get("Bash"), Some(&1));

        // No usage: tools still count, totals don't move.
        let tools_only = line(
            r#"{"type":"assistant","cwd":"/elsewhere","message":{"model":"claude-opus-4-7","content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
        );
        assert!(s.ingest(&tools_only).is_none());
        assert_eq!(s.messages(), 1);
        assert_eq!(s.tool_counts().get("Bash"), Some(&2));
        assert_eq!(s.cwd(), Some("/work/repo"), "the first cwd sticks");
        assert!((s.cost_usd() - cost).abs() < f64::EPSILON);
    }

    #[test]
    fn context_pct_is_capped_and_survives_a_zero_window() {
        let mut s = SessionTotals::default();
        s.ingest(&line(
            r#"{"type":"assistant","message":{"model":"sonnet","content":[],"usage":{"input_tokens":300}}}"#,
        ));
        assert!((s.context_pct(1000) - 0.3).abs() < 1e-9);
        assert!((s.context_pct(100) - 1.0).abs() < f64::EPSILON);
        assert!((s.context_pct(0) - 1.0).abs() < f64::EPSILON);
    }
}
