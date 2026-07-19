//! cctk — shared core for the claude-toolkit tools.
//!
//! Owns the canonical JSONL session-log schema and parser, per-model pricing,
//! pure path/tail helpers, and terminal-free visualization/fuzzy primitives so
//! `ccwatch`, `ccstat`, `ccmap`, and (later) `cctop` all share one core instead
//! of maintaining parallel copies.

pub mod fuzzy;
pub mod jsonl;
pub mod paths;
pub mod pricing;
pub mod viz;

#[cfg(feature = "tail")]
pub mod tail;
