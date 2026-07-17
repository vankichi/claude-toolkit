//! ccwatch — real-time terminal monitor for Claude Code session usage.
//!
//! Library surface: the binary (`main.rs`) is a thin CLI wrapper around
//! [`ui::run`]. JSONL parsing, pricing, and the tail watcher are provided by
//! the shared `cctk` crate.

pub mod session;
pub mod stats;
pub mod summary;
pub mod ui;
