//! Canonical schema and parser for Claude Code's JSONL session log.
//!
//! One [`Line::parse`] per raw line yields a [`Line`]; each consumer then picks
//! the projection it needs:
//! - [`Line::tool_use_names`] — every `tool_use` block name (tool-call counting)
//! - [`Line::extracted`] — model / skill / agent / mcp / command usage records
//! - the plain `model` / `usage` / `timestamp` / `cwd` / `permission_mode` fields
//!
//! This replaces the two divergent parsers previously carried by `ccwatch`
//! (typed `Event`) and `ccstat` (`LineData`/`Extracted`); both now read the
//! same schema through this module.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Token counts for one assistant turn. Flat representation (no nested
/// sub-object) so it is `Copy` + `PartialEq`. `cache_creation_5m` /
/// `cache_creation_1h` split the cache writes by TTL for accurate cost.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_5m: u64,
    pub cache_creation_1h: u64,
}

impl Usage {
    /// Total context size sent to the model on this turn (non-cache input +
    /// cache-creation writes + cache reads).
    #[must_use]
    pub fn context_size(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }

    /// (5min, 1hr) cache-write split. Falls back to (all 5min) when the
    /// per-TTL breakdown was absent in the JSONL.
    #[must_use]
    pub fn cache_creation_split(&self) -> (u64, u64) {
        if self.cache_creation_5m == 0 && self.cache_creation_1h == 0 {
            (self.cache_creation_input_tokens, 0)
        } else {
            (self.cache_creation_5m, self.cache_creation_1h)
        }
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
        self.cache_creation_input_tokens += rhs.cache_creation_input_tokens;
        self.cache_read_input_tokens += rhs.cache_read_input_tokens;
        self.cache_creation_5m += rhs.cache_creation_5m;
        self.cache_creation_1h += rhs.cache_creation_1h;
    }
}

/// Which top-level event a line represents. `assistant` carries usage/model/
/// tools, `user` carries slash-command markers, `permission-mode` carries the
/// current mode. Everything else is `Other` so the schema is forward-compatible.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Assistant,
    User,
    PermissionMode,
    #[default]
    Other,
}

/// One parsed JSONL line. Fields are populated per [`Kind`]; the raw
/// `message.content` is retained privately so the projection methods can
/// derive tool names and usage records on demand.
#[derive(Debug, Default)]
pub struct Line {
    pub kind: Kind,
    /// Raw RFC3339 timestamp string, if present. Parse with [`Line::timestamp_utc`].
    pub timestamp: Option<String>,
    /// Working directory recorded on the event, if present.
    pub cwd: Option<String>,
    /// Model identifier from an `assistant` event's `message.model`.
    pub model: Option<String>,
    /// Token usage from an `assistant` event's `message.usage`, if present.
    pub usage: Option<Usage>,
    /// Raw mode string from a `permission-mode` event
    /// (`default` / `acceptEdits` / `bypassPermissions` / `plan`).
    pub permission_mode: Option<String>,
    content: Option<serde_json::Value>,
}

/// One usage record extracted from a line. A single assistant line can yield a
/// `Model` plus any number of `Skill`/`Agent`/`Mcp` records; a user line yields
/// zero or more `Command` records.
#[derive(Debug, PartialEq)]
pub enum Extracted {
    Model { name: String, usage: Usage },
    Skill { name: String },
    Agent { name: String },
    Mcp { server: String },
    Command { name: String },
}

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    typ: Option<String>,
    message: Option<RawMessage>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<String>,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation: Option<RawCacheCreation>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

impl Line {
    /// Parse one JSONL line. Returns `None` for blank lines and invalid JSON so
    /// callers can `filter_map` over a file without special-casing failures.
    #[must_use]
    pub fn parse(line: &str) -> Option<Line> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        let raw: RawLine = serde_json::from_str(trimmed).ok()?;

        let kind = match raw.typ.as_deref() {
            Some("assistant") => Kind::Assistant,
            Some("user") => Kind::User,
            Some("permission-mode") => Kind::PermissionMode,
            _ => Kind::Other,
        };

        let (model, usage, content) = match raw.message {
            Some(m) => (m.model, m.usage.as_ref().map(to_usage), m.content),
            None => (None, None, None),
        };

        Some(Line {
            kind,
            timestamp: raw.timestamp,
            cwd: raw.cwd,
            model,
            usage,
            permission_mode: raw.permission_mode,
            content,
        })
    }

    /// The event timestamp parsed to UTC, if it was present and well-formed.
    #[must_use]
    pub fn timestamp_utc(&self) -> Option<DateTime<Utc>> {
        self.timestamp
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// Every `tool_use` block name in the message content, in order. Includes
    /// empty names verbatim (callers filter as needed); returns an empty Vec
    /// when the line is not an assistant turn or carries no tool calls.
    #[must_use]
    pub fn tool_use_names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        if let Some(serde_json::Value::Array(blocks)) = &self.content {
            for block in blocks {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                    out.push(
                        block
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                    );
                }
            }
        }
        out
    }

    /// The usage records this line contributes: `Model` (+ its `Usage`) plus
    /// `Skill`/`Agent`/`Mcp` for assistant lines, and `Command` for user lines.
    #[must_use]
    pub fn extracted(&self) -> Vec<Extracted> {
        let mut items = Vec::new();
        match self.kind {
            Kind::Assistant => {
                if let Some(model) = &self.model {
                    items.push(Extracted::Model {
                        name: model.clone(),
                        usage: self.usage.unwrap_or_default(),
                    });
                }
                if let Some(serde_json::Value::Array(blocks)) = &self.content {
                    for block in blocks {
                        if let Some(item) = classify_tool_use(block) {
                            items.push(item);
                        }
                    }
                }
            }
            Kind::User => {
                let text = collect_text(self.content.as_ref());
                for name in scan_commands(&text) {
                    items.push(Extracted::Command { name });
                }
            }
            _ => {}
        }
        items
    }
}

fn to_usage(u: &RawUsage) -> Usage {
    let (m5, h1) = u.cache_creation.as_ref().map_or((0, 0), |c| {
        (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens)
    });
    Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cache_creation_input_tokens: u.cache_creation_input_tokens,
        cache_read_input_tokens: u.cache_read_input_tokens,
        cache_creation_5m: m5,
        cache_creation_1h: h1,
    }
}

/// Classify one `content` block. Returns a usage record for the tool kinds we
/// track (Skill / Agent / MCP), or `None` for text/thinking/other tools.
fn classify_tool_use(block: &serde_json::Value) -> Option<Extracted> {
    if block.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
        return None;
    }
    let name = block
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let input = block.get("input");

    if name == "Skill" {
        let skill = input
            .and_then(|i| i.get("skill"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("(unknown)");
        return Some(Extracted::Skill {
            name: skill.to_string(),
        });
    }

    let subagent = input
        .and_then(|i| i.get("subagent_type"))
        .and_then(serde_json::Value::as_str);
    if name == "Agent" || subagent.is_some() {
        return Some(Extracted::Agent {
            name: subagent.unwrap_or("(default)").to_string(),
        });
    }

    if let Some(server) = mcp_server(name) {
        return Some(Extracted::Mcp { server });
    }

    None
}

/// If `name` is an MCP tool (`mcp__<server>__<tool>`), return `<server>`.
fn mcp_server(name: &str) -> Option<String> {
    let rest = name.strip_prefix("mcp__")?;
    let server = rest.split_once("__").map_or(rest, |(s, _)| s);
    Some(server.to_string())
}

/// Flatten a `message.content` value (string, or array of blocks with `text`)
/// into one string we can scan for command markers.
fn collect_text(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(items)) => {
            let mut out = String::new();
            for item in items {
                match item {
                    serde_json::Value::String(s) => out.push_str(s),
                    serde_json::Value::Object(_) => {
                        if let Some(t) = item.get("text").and_then(serde_json::Value::as_str) {
                            out.push_str(t);
                        }
                    }
                    _ => {}
                }
                out.push('\n');
            }
            out
        }
        _ => String::new(),
    }
}

/// Extract every `<command-name>NAME</command-name>` payload, in order.
fn scan_commands(text: &str) -> Vec<String> {
    const OPEN: &str = "<command-name>";
    const CLOSE: &str = "</command-name>";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        let after = &rest[i + OPEN.len()..];
        let Some(j) = after.find(CLOSE) else { break };
        let name = after[..j].trim();
        if !name.is_empty() {
            out.push(name.to_string());
        }
        rest = &after[j + CLOSE.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(line: &Line) -> Vec<Extracted> {
        line.extracted()
    }

    #[test]
    fn empty_and_invalid_lines_parse_to_none() {
        assert!(Line::parse("").is_none());
        assert!(Line::parse("   ").is_none());
        assert!(Line::parse("not json").is_none());
        assert!(Line::parse("{").is_none());
    }

    #[test]
    fn assistant_yields_model_with_usage_and_timestamp() {
        let line = r#"{"type":"assistant","timestamp":"2026-04-27T04:32:12.600Z","cwd":"/home/u/proj","message":{"model":"claude-opus-4-8","content":[],"usage":{"input_tokens":6,"output_tokens":1103,"cache_creation_input_tokens":100,"cache_read_input_tokens":15206}}}"#;
        let l = Line::parse(line).unwrap();
        assert_eq!(l.kind, Kind::Assistant);
        assert!(l.timestamp_utc().is_some());
        assert_eq!(l.cwd.as_deref(), Some("/home/u/proj"));
        let usage = l.usage.unwrap();
        assert_eq!(usage.output_tokens, 1103);
        assert_eq!(usage.cache_read_input_tokens, 15206);
        assert_eq!(usage.context_size(), 6 + 100 + 15206);
        assert_eq!(l.model.as_deref(), Some("claude-opus-4-8"));
        let ex = items(&l);
        assert_eq!(ex.len(), 1);
        assert!(matches!(&ex[0], Extracted::Model { name, .. } if name == "claude-opus-4-8"));
    }

    #[test]
    fn assistant_extracts_skill_agent_and_mcp_from_tool_use() {
        let line = r#"{"type":"assistant","message":{"model":"sonnet","content":[
            {"type":"text","text":"hi"},
            {"type":"tool_use","name":"Skill","input":{"skill":"superpowers:brainstorming"}},
            {"type":"tool_use","name":"Agent","input":{"subagent_type":"Explore","prompt":"x"}},
            {"type":"tool_use","name":"Agent","input":{"prompt":"no subtype"}},
            {"type":"tool_use","name":"mcp__claude_ai_Notion__notion-fetch","input":{}},
            {"type":"tool_use","name":"Bash","input":{"command":"ls"}}
        ],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let l = Line::parse(line).unwrap();
        let ex = items(&l);
        // Model + Skill + Agent(Explore) + Agent(default) + Mcp(Notion). Bash ignored.
        assert_eq!(ex.len(), 5);
        assert!(ex.contains(&Extracted::Skill {
            name: "superpowers:brainstorming".into()
        }));
        assert!(ex.contains(&Extracted::Agent {
            name: "Explore".into()
        }));
        assert!(ex.contains(&Extracted::Agent {
            name: "(default)".into()
        }));
        assert!(ex.contains(&Extracted::Mcp {
            server: "claude_ai_Notion".into()
        }));
    }

    #[test]
    fn legacy_task_named_dispatch_detected_via_subagent_type() {
        let line = r#"{"type":"assistant","message":{"model":"m","content":[
            {"type":"tool_use","name":"Task","input":{"subagent_type":"general-purpose"}}
        ]}}"#;
        let l = Line::parse(line).unwrap();
        assert!(
            l.extracted()
                .iter()
                .any(|e| matches!(e, Extracted::Agent { name } if name == "general-purpose"))
        );
    }

    #[test]
    fn user_line_extracts_command_markers() {
        let line = r#"{"type":"user","timestamp":"2026-04-27T05:00:00Z","message":{"role":"user","content":"blah <command-name>/review</command-name> more <command-name>/grill-me</command-name>"}}"#;
        let l = Line::parse(line).unwrap();
        assert_eq!(l.kind, Kind::User);
        assert_eq!(
            l.extracted(),
            vec![
                Extracted::Command {
                    name: "/review".into()
                },
                Extracted::Command {
                    name: "/grill-me".into()
                },
            ]
        );
    }

    #[test]
    fn user_line_with_array_content_scans_text_blocks() {
        let line = r#"{"type":"user","message":{"content":[{"type":"text","text":"<command-name>/work-intake</command-name>"}]}}"#;
        let l = Line::parse(line).unwrap();
        assert_eq!(
            l.extracted(),
            vec![Extracted::Command {
                name: "/work-intake".into()
            }]
        );
    }

    #[test]
    fn mcp_server_without_tool_suffix_uses_whole_remainder() {
        assert_eq!(mcp_server("mcp__myserver"), Some("myserver".to_string()));
        assert_eq!(mcp_server("mcp__srv__tool__extra"), Some("srv".to_string()));
        assert_eq!(mcp_server("Bash"), None);
    }

    #[test]
    fn cache_creation_split_falls_back_to_all_5m() {
        let u = Usage {
            cache_creation_input_tokens: 500,
            ..Default::default()
        };
        assert_eq!(u.cache_creation_split(), (500, 0));
        let u2 = Usage {
            cache_creation_5m: 100,
            cache_creation_1h: 200,
            ..Default::default()
        };
        assert_eq!(u2.cache_creation_split(), (100, 200));
    }

    #[test]
    fn cache_creation_sub_object_expands_to_flat_split() {
        let line = r#"{"type":"assistant","message":{"model":"opus","content":[],"usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":1000,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":300,"ephemeral_1h_input_tokens":700}}}}"#;
        let l = Line::parse(line).unwrap();
        let u = l.usage.unwrap();
        assert_eq!(u.cache_creation_5m, 300);
        assert_eq!(u.cache_creation_1h, 700);
        assert_eq!(u.cache_creation_split(), (300, 700));
    }

    #[test]
    fn non_assistant_non_user_events_yield_no_items() {
        let line = r#"{"type":"file-history-snapshot","messageId":"x"}"#;
        let l = Line::parse(line).unwrap();
        assert_eq!(l.kind, Kind::Other);
        assert!(l.extracted().is_empty());
    }

    #[test]
    fn tool_use_names_lists_every_tool_including_empty() {
        let line = r#"{"type":"assistant","message":{"model":"haiku","content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"hi"},{"type":"tool_use","name":"Read"},{"type":"tool_use","name":""}],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let l = Line::parse(line).unwrap();
        assert_eq!(l.tool_use_names(), vec!["Read", ""]);
    }

    #[test]
    fn assistant_without_usage_parses() {
        let line = r#"{"type":"assistant","message":{"model":"sonnet","content":[]}}"#;
        let l = Line::parse(line).unwrap();
        assert!(l.usage.is_none());
        assert_eq!(l.model.as_deref(), Some("sonnet"));
        // Model is still extracted, with default (zero) usage.
        assert_eq!(
            l.extracted(),
            vec![Extracted::Model {
                name: "sonnet".into(),
                usage: Usage::default()
            }]
        );
    }

    #[test]
    fn usage_defaults_missing_fields_to_zero() {
        let line = r#"{"type":"assistant","message":{"model":"sonnet","content":[],"usage":{"input_tokens":5}}}"#;
        let l = Line::parse(line).unwrap();
        let u = l.usage.unwrap();
        assert_eq!(u.input_tokens, 5);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.cache_creation_input_tokens, 0);
        assert_eq!(u.cache_read_input_tokens, 0);
        assert_eq!(u.context_size(), 5);
    }

    #[test]
    fn unknown_type_is_other_kind() {
        let l = Line::parse(r#"{"type":"tool-result","x":1}"#).unwrap();
        assert_eq!(l.kind, Kind::Other);
    }

    #[test]
    fn permission_mode_event_parsed() {
        let l = Line::parse(r#"{"type":"permission-mode","permissionMode":"plan"}"#).unwrap();
        assert_eq!(l.kind, Kind::PermissionMode);
        assert_eq!(l.permission_mode.as_deref(), Some("plan"));
    }

    #[test]
    fn usage_add_assign_accumulates_all_fields() {
        let mut a = Usage {
            input_tokens: 1,
            output_tokens: 2,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 4,
            cache_creation_5m: 5,
            cache_creation_1h: 6,
        };
        a += Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: 30,
            cache_read_input_tokens: 40,
            cache_creation_5m: 50,
            cache_creation_1h: 60,
        };
        assert_eq!(a.input_tokens, 11);
        assert_eq!(a.output_tokens, 22);
        assert_eq!(a.cache_creation_input_tokens, 33);
        assert_eq!(a.cache_read_input_tokens, 44);
        assert_eq!(a.cache_creation_5m, 55);
        assert_eq!(a.cache_creation_1h, 66);
    }
}
