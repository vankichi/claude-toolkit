//! File tail watcher that emits parsed JSONL events on a tokio mpsc channel.
//!
//! Poll-based: every `POLL_INTERVAL`, seek to the last-read offset and read
//! appended bytes. On startup, the existing file is replayed from byte 0 so
//! the UI reflects the in-progress session. On file shrink (rotation), the
//! offset resets to 0.

use crate::jsonl::{self, Event};
use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::mpsc;

const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Start a background tail of `path` on the current tokio runtime. Parsed
/// events flow into `tx` tagged with `tag` so the consumer can dispatch
/// `(idx, Event)` tuples to the right per-session aggregate. The returned
/// `JoinHandle` is `abort()`-able when switching sessions or shutting down.
pub(crate) fn spawn(
    path: PathBuf,
    tag: usize,
    tx: mpsc::Sender<(usize, Event)>,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::spawn(async move { run(path, tag, tx).await })
}

/// Tail loop: open the file every `POLL_INTERVAL`, read any newly appended
/// bytes from the last known offset, split on `\n`, parse, and forward
/// events. Handles file shrink (rotation/replacement) by resetting the
/// offset, and tolerates the file not yet existing on first poll.
async fn run(path: PathBuf, tag: usize, tx: mpsc::Sender<(usize, Event)>) -> Result<()> {
    let mut offset: u64 = 0;
    let mut buf_remainder: Vec<u8> = Vec::new();

    loop {
        match File::open(&path).await {
            Ok(mut f) => {
                let len = f.metadata().await?.len();
                if len < offset {
                    offset = 0;
                    buf_remainder.clear();
                }
                if len > offset {
                    f.seek(SeekFrom::Start(offset)).await?;
                    f.read_to_end(&mut buf_remainder).await?;
                    offset = len;

                    let mut start = 0;
                    for (i, b) in buf_remainder.iter().enumerate() {
                        if *b != b'\n' {
                            continue;
                        }
                        let line = &buf_remainder[start..i];
                        start = i + 1;
                        let Ok(s) = std::str::from_utf8(line) else {
                            continue;
                        };
                        let Ok(Some(ev)) = jsonl::parse_line(s) else {
                            continue;
                        };
                        if tx.send((tag, ev)).await.is_err() {
                            return Ok(());
                        }
                    }
                    buf_remainder.drain(0..start);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::Event;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    const ASSISTANT_LINE: &str = r#"{"type":"assistant","message":{"model":"sonnet","content":[{"type":"tool_use","name":"Bash"}],"usage":{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;

    async fn append(path: &std::path::Path, line: &str) {
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .await
            .unwrap();
        f.write_all(line.as_bytes()).await.unwrap();
        f.write_all(b"\n").await.unwrap();
        f.flush().await.unwrap();
    }

    async fn next_event(rx: &mut mpsc::Receiver<(usize, Event)>) -> (usize, Event) {
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("watcher did not produce an event within 3s")
            .expect("channel closed")
    }

    #[tokio::test]
    async fn tails_appended_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        // Pre-create empty file so the watcher has something to read.
        tokio::fs::write(&path, b"").await.unwrap();

        let (tx, mut rx) = mpsc::channel::<(usize, Event)>(8);
        let handle = spawn(path.clone(), 7, tx);

        append(&path, ASSISTANT_LINE).await;
        let (tag, ev) = next_event(&mut rx).await;
        assert_eq!(tag, 7);
        assert!(matches!(ev, Event::Assistant(_)));

        append(&path, ASSISTANT_LINE).await;
        let (tag, ev) = next_event(&mut rx).await;
        assert_eq!(tag, 7);
        assert!(matches!(ev, Event::Assistant(_)));

        handle.abort();
    }

    #[tokio::test]
    async fn handles_partial_line_across_polls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        tokio::fs::write(&path, b"").await.unwrap();

        let (tx, mut rx) = mpsc::channel::<(usize, Event)>(8);
        let handle = spawn(path.clone(), 0, tx);

        // Write the line in two halves without the newline first.
        let half = ASSISTANT_LINE.len() / 2;
        let first = &ASSISTANT_LINE[..half];
        let second = &ASSISTANT_LINE[half..];
        {
            let mut f = tokio::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .await
                .unwrap();
            f.write_all(first.as_bytes()).await.unwrap();
            f.flush().await.unwrap();
        }
        // Give the watcher a chance to read the partial bytes.
        tokio::time::sleep(Duration::from_millis(300)).await;
        // No event should have been produced yet.
        assert!(rx.try_recv().is_err());

        append(&path, second).await;
        let (_, ev) = next_event(&mut rx).await;
        assert!(matches!(ev, Event::Assistant(_)));

        handle.abort();
    }

    #[tokio::test]
    async fn waits_for_file_to_be_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-yet.jsonl");

        let (tx, mut rx) = mpsc::channel::<(usize, Event)>(8);
        let handle = spawn(path.clone(), 0, tx);

        // Slight delay so the watcher polls the missing file at least once.
        tokio::time::sleep(Duration::from_millis(300)).await;
        append(&path, ASSISTANT_LINE).await;

        let (_, ev) = next_event(&mut rx).await;
        assert!(matches!(ev, Event::Assistant(_)));
        handle.abort();
    }
}
