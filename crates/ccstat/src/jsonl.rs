//! Parser that extracts usage records from one Claude Code JSONL line.
//!
//! We care about two event shapes: `assistant` (carries the model, token
//! `usage`, and `tool_use` blocks whose inputs name the skill/agent/MCP
//! server invoked) and `user` (carries `<command-name>` markers for slash
//! commands the human typed). Every other event and every parse failure
//! collapses to an empty [`LineData`], so the parser is forward-compatible.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Token counts for one assistant turn (subset of the API `usage` object),
/// with the cache-write TTL split needed for accurate cost.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_5m: u64,
    pub cache_creation_1h: u64,
}

impl Usage {
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

/// One usage record extracted from a line. A single assistant line can yield
/// a `Model` plus any number of `Skill`/`Agent`/`Mcp` records; a user line
/// yields zero or more `Command` records.
#[derive(Debug, PartialEq)]
pub enum Extracted {
    Model { name: String, usage: Usage },
    Skill { name: String },
    Agent { name: String },
    Mcp { server: String },
    Command { name: String },
}

/// Everything one line contributes: its timestamp and cwd (for bucketing and
/// project attribution) plus the extracted usage records.
#[derive(Debug, Default)]
pub struct LineData {
    pub timestamp: Option<DateTime<Utc>>,
    pub cwd: Option<String>,
    pub items: Vec<Extracted>,
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

/// Parse one JSONL line into the usage records it contributes. Returns an
/// empty `LineData` for blank lines, invalid JSON, and event types we don't
/// track.
#[must_use]
pub fn parse_line(line: &str) -> LineData {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return LineData::default();
    }
    let Ok(raw) = serde_json::from_str::<RawLine>(trimmed) else {
        return LineData::default();
    };

    let timestamp = raw
        .timestamp
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let mut data = LineData {
        timestamp,
        cwd: raw.cwd.clone(),
        items: Vec::new(),
    };

    match raw.typ.as_deref() {
        Some("assistant") => {
            if let Some(msg) = &raw.message {
                if let Some(model) = &msg.model {
                    data.items.push(Extracted::Model {
                        name: model.clone(),
                        usage: to_usage(msg.usage.as_ref()),
                    });
                }
                if let Some(serde_json::Value::Array(blocks)) = &msg.content {
                    for block in blocks {
                        if let Some(item) = classify_tool_use(block) {
                            data.items.push(item);
                        }
                    }
                }
            }
        }
        Some("user") => {
            if let Some(msg) = &raw.message {
                let text = collect_text(msg.content.as_ref());
                for name in scan_commands(&text) {
                    data.items.push(Extracted::Command { name });
                }
            }
        }
        _ => {}
    }

    data
}

fn to_usage(raw: Option<&RawUsage>) -> Usage {
    let Some(u) = raw else {
        return Usage::default();
    };
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

    fn kinds(d: &LineData) -> Vec<&Extracted> {
        d.items.iter().collect()
    }

    #[test]
    fn empty_and_invalid_lines_yield_no_items() {
        assert!(parse_line("").items.is_empty());
        assert!(parse_line("   ").items.is_empty());
        assert!(parse_line("not json").items.is_empty());
        assert!(parse_line("{").items.is_empty());
    }

    #[test]
    fn assistant_yields_model_with_usage_and_timestamp() {
        let line = r#"{"type":"assistant","timestamp":"2026-04-27T04:32:12.600Z","cwd":"/home/u/proj","message":{"model":"claude-opus-4-8","content":[],"usage":{"input_tokens":6,"output_tokens":1103,"cache_creation_input_tokens":100,"cache_read_input_tokens":15206}}}"#;
        let d = parse_line(line);
        assert!(d.timestamp.is_some());
        assert_eq!(d.cwd.as_deref(), Some("/home/u/proj"));
        assert_eq!(d.items.len(), 1);
        let Extracted::Model { name, usage } = &d.items[0] else {
            panic!()
        };
        assert_eq!(name, "claude-opus-4-8");
        assert_eq!(usage.output_tokens, 1103);
        assert_eq!(usage.cache_read_input_tokens, 15206);
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
        let d = parse_line(line);
        // Model + Skill + Agent(Explore) + Agent(default) + Mcp(Notion). Bash ignored.
        assert_eq!(d.items.len(), 5);
        assert!(kinds(&d).contains(&&Extracted::Skill {
            name: "superpowers:brainstorming".into()
        }));
        assert!(kinds(&d).contains(&&Extracted::Agent {
            name: "Explore".into()
        }));
        assert!(kinds(&d).contains(&&Extracted::Agent {
            name: "(default)".into()
        }));
        assert!(kinds(&d).contains(&&Extracted::Mcp {
            server: "claude_ai_Notion".into()
        }));
    }

    #[test]
    fn legacy_task_named_dispatch_detected_via_subagent_type() {
        let line = r#"{"type":"assistant","message":{"model":"m","content":[
            {"type":"tool_use","name":"Task","input":{"subagent_type":"general-purpose"}}
        ]}}"#;
        let d = parse_line(line);
        assert!(
            d.items.contains(&Extracted::Agent {
                name: "general-purpose".into()
            }) || d
                .items
                .iter()
                .any(|e| matches!(e, Extracted::Agent { name } if name == "general-purpose"))
        );
    }

    #[test]
    fn user_line_extracts_command_markers() {
        let line = r#"{"type":"user","timestamp":"2026-04-27T05:00:00Z","message":{"role":"user","content":"blah <command-name>/review</command-name> more <command-name>/grill-me</command-name>"}}"#;
        let d = parse_line(line);
        assert_eq!(
            d.items,
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
        let d = parse_line(line);
        assert_eq!(
            d.items,
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
    fn non_assistant_non_user_events_ignored() {
        let line = r#"{"type":"file-history-snapshot","messageId":"x"}"#;
        assert!(parse_line(line).items.is_empty());
    }
}
