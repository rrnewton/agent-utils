//! Headless agents: a harness driven over its structured protocol instead of a terminal
//! (`#565 headless-coordinator`; design in
//! `ai_docs/transient/2026-10-26-headless-coordinator/README.md`).
//!
//! - [`events`]: the typed JSONL event log a headless agent writes (M1).
//! - [`tail`]: `agentctl tail NAME`, a read-only follower of that log (M1).
//!
//! The log sits in the agent's registry directory beside its record:
//!
//! ```text
//! <registry>/<agent>/events/         the event log
//! ```

pub mod events;
pub mod tail;
