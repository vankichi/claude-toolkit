//! Look up human session names from `claude agents --json`.
//!
//! The JSONL logs only carry a session UUID; the friendly name a user sees
//! comes from the `claude` CLI's agent listing. This is best-effort: any
//! failure (CLI missing, non-zero exit, timeout, parse error) yields an empty
//! map, and cctop falls back to the short session id.

use std::collections::HashMap;
use std::time::Duration;

/// Hard cap on how long we wait for `claude agents --json`.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Map of session id → display name for currently-listed agents. Sessions
/// without a non-empty name are omitted (the caller falls back to the id).
pub async fn names() -> HashMap<String, String> {
    let fut = tokio::process::Command::new("claude")
        .args(["agents", "--json"])
        .stdin(std::process::Stdio::null())
        .output();

    let Ok(Ok(output)) = tokio::time::timeout(TIMEOUT, fut).await else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }

    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    if let Some(entries) = value.as_array() {
        for entry in entries {
            let id = entry.get("sessionId").and_then(serde_json::Value::as_str);
            let name = entry
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty());
            if let (Some(id), Some(name)) = (id, name) {
                map.insert(id.to_string(), name.to_string());
            }
        }
    }
    map
}
