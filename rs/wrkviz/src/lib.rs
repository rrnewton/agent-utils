//! The Rust edition of `wrkviz`, the durable agent-team timeline
//! archive tool.
//!
//! The Python edition (`py/wrkviz`, ~60k lines) is the oracle.
//! This crate grows subcommand by subcommand at differential
//! parity, the way `rs/wrkslotsd` grew behind wrkslots. Slice 1
//! (#706 rust-primary-wrkviz) ports `inspect` only: the schema-3
//! generation acceptance rules, the six collection counts, the
//! schema-1 fallback, and the snapshot pointer summary, over a
//! corpus the synthetic generator builds deterministically.
//!
//! Format ownership rule for the milestone: the archive record
//! types defined here (the schema-3 bootstrap and shard
//! catalogue first) are the single source of truth; the Python
//! mirror validates against them rather than re-deriving them.

#![forbid(unsafe_code)]

/// The CLI surface: `wrkviz inspect --output <archive>`.
pub mod cli;
/// The `inspect` subcommand and the schema-3 archive reader it
/// is built on.
pub mod inspect;
/// The archive JSON value domain and the Python-shaped readers
/// over it.
pub mod json;
/// The `query list` subcommand over schema-3 and schema-1
/// timelines.
pub mod query;
/// The snapshot pointer (`snapshot-root.json`) and the legacy
/// in-archive snapshot layout.
pub mod snapshot_store;
