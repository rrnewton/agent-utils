//! Allowlisted, audited command execution through an out-of-sandbox Herdr pane.
//!
//! The crate exposes the policy and transport layers used by the `herdr-run` command so callers
//! can validate configuration and admission without starting a Herdr session.

pub mod agent;
pub mod agent_cli;
pub mod allowlist;
pub mod audit;
pub mod cli;
pub mod client;
pub mod config;
pub mod error;
pub mod identity;
pub mod readiness;
pub mod reap;
pub mod retention;
pub mod runner;
pub mod session;
pub mod sweep;

mod state;
mod timefmt;

/// Full installed-package reference, also printed by `herdr-run userguide`.
pub const USER_GUIDE: &str = include_str!("embedded_userguide.md");

/// Full installed messaging reference, also printed by `herdr-agent userguide`.
pub const AGENT_USER_GUIDE: &str = include_str!("embedded_agent_userguide.md");
