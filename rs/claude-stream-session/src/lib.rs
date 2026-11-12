//! Persistent Claude Code stream-json sessions, shared by the tools in this repository.
//!
//! Factored out of the headless driver (`#1331 claude-session-summarizer`) with no
//! behaviour change for its existing callers: the driver's protocol module re-exports the
//! [`protocol`] helpers from here, so every existing call site keeps its path and meaning.
//!
//! - [`protocol`]: the stateless stream-json framing — protocol arguments, permission-prompt
//!   argument handling, reserved-argument validation, UUIDs, and stdin lines.
//! - [`session`]: one persistent `claude -p --input-format stream-json` process driven turn
//!   by turn, with restart on death. Callers that need a pool build it over [`session::Session`].
//!
//! This crate only transports. It knows nothing about summaries, queues, or registries.

pub mod protocol;
pub mod session;
