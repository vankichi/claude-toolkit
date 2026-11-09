//! ccwatch — real-time terminal monitor for Claude Code session usage.
//!
//! Library surface: the binary (`main.rs`) is a thin CLI wrapper around
//! [`ui::run`]. JSONL parsing, pricing, and the tail watcher are provided by
//! the shared `cctk` crate.

mod session;
mod stats;
mod summary;
pub mod ui;
