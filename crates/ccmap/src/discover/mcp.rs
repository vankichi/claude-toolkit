//! MCP server discovery: extracts server *names only* from `~/.claude.json`
//! and `<project_dir>/.mcp.json`. This is deliberately secret-safe —
//! `~/.claude.json` also carries OAuth tokens and per-server `command`/
//! `args`/`env` values, none of which this module may ever materialize or
//! print. See the `ClaudeJson`/`ProjectEntry`/`McpJson` shapes below: every
//! `mcpServers` map is typed with [`serde::de::IgnoredAny`] values, so serde
//! parses far enough to keep the server-name keys but discards the value
//! bytes without ever building a `String`/`Value` out of them. Fields this
//! module has no use for (notably `oauthAccount`) are simply absent from
//! the types, so they are never read either.

use crate::model::{Item, Kind, Source};
use serde::de::{DeserializeOwned, IgnoredAny};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, serde::Deserialize)]
struct ClaudeJson {
    // Security-mandated shape: a `BTreeSet<String>` would parse a JSON
    // array, not the `{"name": {...}}` object Claude Code actually writes.
    // `IgnoredAny` is the only zero-sized value type that both accepts an
    // arbitrary value shape (command/args/env/token) and guarantees it is
    // never materialized.
    #[allow(clippy::zero_sized_map_values)]
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, IgnoredAny>,
    #[serde(default)]
    projects: BTreeMap<String, ProjectEntry>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ProjectEntry {
    #[allow(clippy::zero_sized_map_values)]
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, IgnoredAny>,
    #[serde(default, rename = "enabledMcpjsonServers")]
    enabled_mcpjson: Vec<String>,
    #[serde(default, rename = "disabledMcpjsonServers")]
    disabled_mcpjson: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct McpJson {
    #[allow(clippy::zero_sized_map_values)]
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, IgnoredAny>,
}

/// Reads and parses `path` as JSON into `T`, returning `T::default()` if the
/// file is missing or fails to parse. Never panics on external input.
fn read_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Builds a name-only `Kind::Mcp` item, recording enable/disable lifecycle
/// state in `extra` since MCP servers have no [`crate::model::PluginState`].
fn mcp_item(name: &str, source: Source, state: &str) -> Item {
    Item {
        kind: Kind::Mcp,
        name: name.to_string(),
        description: String::new(),
        source,
        path: None,
        extra: vec![("state".to_string(), state.to_string())],
        plugin_state: None,
    }
}

/// Resolves the enable/disable state of a project-scope (`.mcp.json`)
/// server name against the matching project entry's allow/deny lists.
/// `disabledMcpjsonServers` wins over `enabledMcpjsonServers` if a name
/// were ever (incorrectly) listed in both. A name in neither list is
/// `"active"` — user/local names are always `"active"` for the same reason.
fn project_scope_state(name: &str, enabled: &[String], disabled: &[String]) -> &'static str {
    if disabled.iter().any(|s| s == name) {
        "disabled"
    } else if enabled.iter().any(|s| s == name) {
        "enabled"
    } else {
        "active"
    }
}

/// Discovers MCP server *names* across the three scopes Claude Code
/// recognizes:
/// - **user**: top-level `mcpServers` in `claude_json` → [`Source::User`].
/// - **local**: `projects[canonical(project_dir)].mcpServers` in
///   `claude_json` → [`Source::Project`]. Every other project entry is left
///   untouched (only the one matching key is ever indexed).
/// - **project**: `<project_dir>/.mcp.json`'s `mcpServers` → also
///   [`Source::Project`], with enable/disable state taken from the matching
///   project entry's `enabledMcpjsonServers`/`disabledMcpjsonServers`.
///
/// A missing or unparsable `claude_json`/`.mcp.json` is treated as empty
/// rather than an error. `project_dir` is canonicalized for the `projects`
/// lookup only; if canonicalization fails (e.g. the directory doesn't
/// exist), the raw path is used instead so the lookup can still (fail to)
/// match rather than erroring out.
#[must_use]
pub fn discover(claude_json: &Path, project_dir: &Path) -> Vec<Item> {
    let claude_json: ClaudeJson = read_or_default(claude_json);
    let canonical_project_dir =
        fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let project_entry = claude_json
        .projects
        .get(canonical_project_dir.to_string_lossy().as_ref());

    let mut items = Vec::new();

    for name in claude_json.mcp_servers.keys() {
        items.push(mcp_item(name, Source::User, "active"));
    }

    if let Some(entry) = project_entry {
        for name in entry.mcp_servers.keys() {
            items.push(mcp_item(name, Source::Project, "active"));
        }
    }

    let empty: Vec<String> = Vec::new();
    let (enabled, disabled) = project_entry.map_or((&empty, &empty), |entry| {
        (&entry.enabled_mcpjson, &entry.disabled_mcpjson)
    });
    let project_mcp_json: McpJson = read_or_default(&project_dir.join(".mcp.json"));
    for name in project_mcp_json.mcp_servers.keys() {
        let state = project_scope_state(name, enabled, disabled);
        items.push(mcp_item(name, Source::Project, state));
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn discovers_user_local_project_scopes_with_enable_disable_state_and_no_secrets() {
        let tmp = tempfile::tempdir().unwrap();

        let project_dir = tmp.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();
        let canonical_project = fs::canonicalize(&project_dir).unwrap();

        let other_project_dir = tmp.path().join("other-project");
        fs::create_dir_all(&other_project_dir).unwrap();
        let canonical_other = fs::canonicalize(&other_project_dir).unwrap();

        let mut projects = serde_json::Map::new();
        projects.insert(
            canonical_project.to_string_lossy().into_owned(),
            serde_json::json!({
                "mcpServers": { "localsrv": { "command": "also-secret-value" } },
                "disabledMcpjsonServers": ["projsrv"]
            }),
        );
        projects.insert(
            canonical_other.to_string_lossy().into_owned(),
            serde_json::json!({
                "mcpServers": { "othersrv": { "command": "unrelated-secret" } }
            }),
        );
        let claude_json_value = serde_json::json!({
            "mcpServers": { "usersrv": { "command": "secret-token-xyz" } },
            "projects": serde_json::Value::Object(projects)
        });
        let claude_json_path = tmp.path().join("claude.json");
        fs::write(
            &claude_json_path,
            serde_json::to_string_pretty(&claude_json_value).unwrap(),
        )
        .unwrap();

        fs::write(
            project_dir.join(".mcp.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": { "projsrv": {} }
            }))
            .unwrap(),
        )
        .unwrap();

        let items = discover(&claude_json_path, &project_dir);

        let names: BTreeSet<&str> = items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(
            names,
            BTreeSet::from_iter(["usersrv", "localsrv", "projsrv"])
        );
        assert!(items.iter().all(|item| item.kind == Kind::Mcp));

        let usersrv = items.iter().find(|item| item.name == "usersrv").unwrap();
        assert_eq!(usersrv.source, Source::User);
        assert!(
            usersrv
                .extra
                .contains(&("state".to_string(), "active".to_string()))
        );

        let localsrv = items.iter().find(|item| item.name == "localsrv").unwrap();
        assert_eq!(localsrv.source, Source::Project);
        assert!(
            localsrv
                .extra
                .contains(&("state".to_string(), "active".to_string()))
        );

        let projsrv = items.iter().find(|item| item.name == "projsrv").unwrap();
        assert_eq!(projsrv.source, Source::Project);
        assert!(
            projsrv
                .extra
                .contains(&("state".to_string(), "disabled".to_string()))
        );

        let debug_output = format!("{items:?}");
        assert!(!debug_output.contains("secret-token-xyz"));
        assert!(!debug_output.contains("also-secret-value"));
        assert!(!debug_output.contains("unrelated-secret"));
    }

    #[test]
    fn missing_claude_json_and_mcp_json_yield_no_items_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();

        let items = discover(&tmp.path().join("does-not-exist.json"), &project_dir);

        assert!(items.is_empty());
    }

    #[test]
    fn malformed_claude_json_is_treated_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();
        let claude_json_path = tmp.path().join("claude.json");
        fs::write(&claude_json_path, "{ not valid json").unwrap();

        let items = discover(&claude_json_path, &project_dir);

        assert!(items.is_empty());
    }
}
