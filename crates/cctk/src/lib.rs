//! cctk — shared core for the claude-toolkit tools.
//!
//! Owns the canonical JSONL session-log schema and parser, per-model pricing,
//! pure path/tail helpers, ratatui chart / text visualization helpers, and
//! fuzzy matching, so `ccwatch`, `ccstat`, `ccmap`, `cctop` and `ccfocus`
//! share one core instead of maintaining parallel copies.

#[cfg(feature = "tail")]
pub mod agents;
pub mod chart;
#[cfg(feature = "cli")]
pub mod cli;
pub mod cycle;
pub mod fmt;
pub mod focus;
pub mod fuzzy;
pub mod jsonl;
pub mod paths;
pub mod pricing;
pub mod session;
pub mod style;
pub mod term;
pub mod viz;

#[cfg(feature = "tail")]
pub mod tail;
