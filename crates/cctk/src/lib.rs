//! cctk — shared core for the claude-toolkit tools.
//!
//! Owns the canonical JSONL session-log schema and parser, per-model pricing,
//! and pure path/tail helpers so `ccwatch`, `ccstat`, and (later) `cctop` all
//! read the same data the same way instead of maintaining parallel copies.
