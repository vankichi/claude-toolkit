//! Running `claude agents --json`, the `claude` CLI's list of live sessions.
//! Callers parse the JSON themselves: each keeps the fields it shows and
//! decides what a failure means for it.

use anyhow::{Context, Result};
use std::time::Duration;

/// How long to wait for the CLI. It normally answers at once from local
/// state; this only keeps a hung child from stalling a TUI.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// The CLI's stdout (a JSON array of `{ sessionId, name?, status, … }`).
///
/// # Errors
/// When `claude` is not on PATH or cannot be spawned, does not answer
/// within [`TIMEOUT`], or exits non-zero (the message carries its stderr).
pub async fn json() -> Result<Vec<u8>> {
    let fut = tokio::process::Command::new("claude")
        .args(["agents", "--json"])
        .stdin(std::process::Stdio::null())
        // On a timeout the future is dropped; without this the CLI runs on.
        .kill_on_drop(true)
        .output();

    let output = match tokio::time::timeout(TIMEOUT, fut).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "could not run `claude agents --json`: the `claude` CLI was not found on PATH"
            );
        }
        Ok(Err(e)) => return Err(e).context("spawning `claude agents --json`"),
        Err(_) => anyhow::bail!(
            "`claude agents --json` timed out after {}s",
            TIMEOUT.as_secs()
        ),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "`claude agents --json` exited with {}: {}",
            output.status,
            stderr.trim()
        );
    }
    Ok(output.stdout)
}
