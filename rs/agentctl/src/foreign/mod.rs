//! Headless-worker runtime for foreign harnesses.
//!
//! Track B step B3 (see
//! `ai_docs/transient/2026-11-07-rust-primary/herdr-subagents-port-map.md`).
//! Slice B3-1 covers the shared ownership/identity core types in
//! [`ownership`]; slice B3-2 adds the kernel evidence they are checked
//! against ([`procstat`] plus the procfs readers in [`ownership`]); slice
//! B3-3 adds pidfd pinning and `stop_process` in [`ownership`]; slice B3-4
//! adds the registry layout, active-harness bridge, records, and locked
//! persistence in [`registry`]; slice B3-5 adds cgroup helpers and
//! harness-tree cleanup in [`ownership`]; slice B3-6 adds owned spawning
//! in [`ownership`] and the runner dispatch core in [`runner`]. No CLI
//! wiring yet.

pub mod lifecycle;
pub mod ownership;
pub mod procstat;
pub mod registry;
pub mod runner;
