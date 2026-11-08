//! Headless-worker runtime for foreign harnesses.
//!
//! Track B step B3 (see
//! `ai_docs/transient/2026-11-07-rust-primary/herdr-subagents-port-map.md`).
//! Slice B3-1 covers the shared ownership/identity core types in
//! [`ownership`]; slice B3-2 adds the kernel evidence they are checked
//! against ([`procstat`] plus the procfs readers in [`ownership`]). No CLI
//! wiring yet.

pub mod ownership;
pub mod procstat;
