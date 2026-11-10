//! Look up human session names from `claude agents --json`.
//!
//! The JSONL logs only carry a session UUID; the friendly name a user sees
//! comes from the `claude` CLI's agent listing. This is best-effort: any
//! failure (CLI missing, non-zero exit, timeout, parse error) yields an empty
//! map, and cctop falls back to the short session id.

use std::collections::HashMap;

/// Map of session id → display name for currently-listed agents. Sessions
/// without a non-empty name are omitted (the caller falls back to the id).
pub async fn names() -> HashMap<String, String> {
    let Ok(stdout) = cctk::agents::json().await else {
        return HashMap::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&stdout) else {
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
