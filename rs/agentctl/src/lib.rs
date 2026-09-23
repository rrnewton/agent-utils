//! Persistent coding-agent sessions, independent of shell execution.
//!
//! The Herdr adapter provides native interactive sessions and durable prompt delivery.
//! Chat and headless adapters are supplied by the optional runtime extensions.

pub mod agent;
pub(crate) mod chat_events;
pub mod chat_runtime;
pub mod chat_service;
pub mod cli;
pub mod client;
pub mod codex_goal;
pub mod error;
pub mod legacy_cli;
pub mod plugins;
pub(crate) mod profiles;
pub(crate) mod skill_install;
pub mod subagents;

/// Reference for the canonical command and its supported adapters.
pub const USER_GUIDE: &str = include_str!("embedded_userguide.md");
/// One-screen introduction to managing visible workers.
pub const QUICKSTART: &str = include_str!("embedded_quickstart.md");
/// Compatibility reference for the former messaging command.
pub const AGENT_USER_GUIDE: &str = include_str!("embedded_agent_userguide.md");
/// Operator reference for the Rust chat subscription service.
pub const CHAT_USER_GUIDE: &str = include_str!("embedded_chat_userguide.md");
/// One-screen setup for the Rust chat subscription service.
pub const CHAT_QUICKSTART: &str = include_str!("embedded_chat_quickstart.md");
/// Harness skill installed by `agentctl skill install`.
pub const AGENTCTL_SKILL: &str = include_str!("embedded_agentctl_skill.md");
