//! ccfocus — hook-driven status board of every Claude Code session.
//!
//! `ccfocus hook <event>` is registered as a Claude Code command hook. It
//! reads the hook's JSON from stdin and records the session's state in
//! `<state>/sessions/<session_id>.json` (schema in [`cctk::focus`]) plus one
//! line in `<state>/events/<YYYYMMDD>.tsv`. Prompt and message bodies are
//! never stored. All writes live in this crate; `cctk` only reads.

pub mod app;
mod board;
mod cmd;
mod config;
mod events;
mod fsx;
mod hook;
mod job;
pub mod probe;
mod prune;
mod repos;
mod text;
mod tmux;
mod transcript;
pub mod tui;
pub mod ui;

pub use hook::{Event, Outcome, handle};
