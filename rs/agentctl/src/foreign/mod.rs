//! Headless-worker runtime for foreign harnesses.
//!
//! Track B step B3 (see
//! `ai_docs/transient/2026-11-07-rust-primary/herdr-subagents-port-map.md`).
//! Slice B3-1 covers only the shared ownership/identity core types in
//! [`ownership`]; no CLI wiring yet.

pub mod ownership;
