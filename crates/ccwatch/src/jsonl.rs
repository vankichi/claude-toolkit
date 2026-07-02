//! Schema definitions and parser for Claude Code's JSONL session log.
//!
//! Only the `assistant` event variant carries everything we care about
//! (model, usage, `tool_use` blocks, cwd, timestamp). All other event types
//! collapse into `Event::Other` so future Claude Code updates that introduce
//! new event types don't break the parser.

use serde::Deserialize;

/// One line in the JSONL session log. Discriminated by the top-level `type`
/// field. `assistant` carries the usage metrics; `permission-mode` carries
/// the current agent mode (default / plan / acceptEdits / bypassPermissions).
/// Anything else collapses to `Other` so the parser is forward-compat.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum Event {
    Assistant(AssistantEvent),
    PermissionMode(PermissionModeEvent),
    #[serde(other)]
    Other,
}

/// `permission-mode` event payload. Emitted when the user switches the
/// session's permission mode (e.g. `default` → `plan` via `/plan`).
#[derive(Debug, Deserialize)]
pub(crate) struct PermissionModeEvent {
    /// Raw mode string from the JSONL. Known values:
    /// `default`, `acceptEdits`, `bypassPermissions`, `plan`.
    #[serde(rename = "permissionMode")]
    pub permission_mode: String,
}

/// A single assistant turn captured in the JSONL log. Wraps the API's
/// response payload (`message`) plus session-scoped context (`cwd`, `timestamp`).
#[derive(Debug, Deserialize)]
pub(crate) struct AssistantEvent {
    pub message: AssistantMessage,
    /// Working directory recorded at the time of the event. Same for every
    /// event in a session; we only need to capture it once.
    #[serde(default)]
    pub cwd: Option<String>,
    /// RFC3339 timestamp of the event. Used for sliding-window rate calculations
    /// and active-session detection.
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// Subset of the assistant API response we need: which model produced the
/// reply, what content blocks (tool calls) it emitted, and the token usage.
#[derive(Debug, Deserialize)]
pub(crate) struct AssistantMessage {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// One block inside `AssistantMessage::content`. We only care about
/// `tool_use` (to count tool invocations); text/thinking/etc. collapse into
/// `Other` so the schema is forward-compatible.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlock {
    ToolUse {
        #[serde(default)]
        name: String,
    },
    #[serde(other)]
    Other,
}

/// Token counts for one assistant turn. The four primary fields cover
/// non-cache input, output, cache-creation writes, and cache reads.
/// `cache_creation` is the optional sub-object that splits the writes by TTL.
#[derive(Debug, Default, Deserialize, Clone, Copy)]
pub(crate) struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Breakdown of `cache_creation_input_tokens` by TTL. Anthropic prices 1h
    /// cache writes higher (2x input) than 5min (1.25x input). When this field
    /// is absent, all `cache_creation` tokens are treated as 5min.
    #[serde(default)]
    pub cache_creation: Option<CacheCreation>,
}

/// Per-TTL breakdown of `cache_creation_input_tokens`. Anthropic charges
/// 1hr-ephemeral writes at 2× input rate vs 1.25× for 5min, so we need both
/// to compute cost accurately.
#[derive(Debug, Default, Deserialize, Clone, Copy)]
pub(crate) struct CacheCreation {
    #[serde(default)]
    pub ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub ephemeral_1h_input_tokens: u64,
}

impl Usage {
    /// Total context size sent to the model on this turn.
    #[must_use]
    pub fn context_size(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }

    /// (5min, 1hr) `cache_creation` breakdown. Falls back to (all 5min) if the
    /// JSONL didn't carry the `cache_creation` sub-object.
    #[must_use]
    pub fn cache_creation_split(&self) -> (u64, u64) {
        match self.cache_creation {
            Some(c) => (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens),
            None => (self.cache_creation_input_tokens, 0),
        }
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
        self.cache_creation_input_tokens += rhs.cache_creation_input_tokens;
        self.cache_read_input_tokens += rhs.cache_read_input_tokens;
        // `cache_creation` only matters for cost (computed per-event before
        // aggregation), so we don't accumulate it on the totals struct.
    }
}

/// Parse a single JSONL line. Returns Ok(None) if the line is empty or unrecognized.
pub(crate) fn parse_line(line: &str) -> serde_json::Result<Option<Event>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let ev: Event = serde_json::from_str(trimmed)?;
    Ok(Some(ev))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assistant_with_usage() {
        let line = r#"{"type":"assistant","message":{"model":"claude-opus-4-7","content":[{"type":"tool_use","name":"Bash"}],"usage":{"input_tokens":6,"output_tokens":1103,"cache_creation_input_tokens":9667,"cache_read_input_tokens":15206}},"timestamp":"2026-04-27T04:32:12.600Z"}"#;
        let ev = parse_line(line).unwrap().unwrap();
        let Event::Assistant(a) = ev else {
            panic!("not assistant")
        };
        let usage = a.message.usage.unwrap();
        assert_eq!(usage.output_tokens, 1103);
        assert_eq!(usage.cache_read_input_tokens, 15206);
        assert_eq!(usage.context_size(), 6 + 9667 + 15206);
        assert_eq!(a.message.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(a.message.content.len(), 1);
    }

    #[test]
    fn ignores_unknown_type() {
        let line = r#"{"type":"file-history-snapshot","messageId":"x"}"#;
        let ev = parse_line(line).unwrap().unwrap();
        assert!(matches!(ev, Event::Other));
    }

    #[test]
    fn empty_line_returns_none() {
        assert!(parse_line("").unwrap().is_none());
        assert!(parse_line("   \n").unwrap().is_none());
    }

    #[test]
    fn assistant_without_usage_parses() {
        let line = r#"{"type":"assistant","message":{"model":"sonnet","content":[]}}"#;
        let ev = parse_line(line).unwrap().unwrap();
        let Event::Assistant(a) = ev else {
            panic!("not assistant")
        };
        assert!(a.message.usage.is_none());
        assert_eq!(a.message.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn tolerates_thinking_and_text_content_blocks() {
        let line = r#"{"type":"assistant","message":{"model":"haiku","content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"hi"},{"type":"tool_use","name":"Read"}],"usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;
        let ev = parse_line(line).unwrap().unwrap();
        let Event::Assistant(a) = ev else { panic!() };
        assert_eq!(a.message.content.len(), 3);
        // Only the tool_use block should be the named variant; the rest are Other.
        let tool_uses: Vec<_> = a
            .message
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { name } => Some(name.as_str()),
                ContentBlock::Other => None,
            })
            .collect();
        assert_eq!(tool_uses, vec!["Read"]);
    }

    #[test]
    fn invalid_json_returns_err() {
        assert!(parse_line("not json at all").is_err());
        assert!(parse_line("{").is_err());
    }

    #[test]
    fn user_event_classified_as_other() {
        let line = r#"{"type":"user","message":{"role":"user","content":"hi"}}"#;
        let ev = parse_line(line).unwrap().unwrap();
        assert!(matches!(ev, Event::Other));
    }

    #[test]
    fn usage_defaults_missing_fields_to_zero() {
        let line = r#"{"type":"assistant","message":{"model":"sonnet","content":[],"usage":{"input_tokens":5}}}"#;
        let ev = parse_line(line).unwrap().unwrap();
        let Event::Assistant(a) = ev else { panic!() };
        let u = a.message.usage.unwrap();
        assert_eq!(u.input_tokens, 5);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.cache_creation_input_tokens, 0);
        assert_eq!(u.cache_read_input_tokens, 0);
        assert_eq!(u.context_size(), 5);
    }
}
