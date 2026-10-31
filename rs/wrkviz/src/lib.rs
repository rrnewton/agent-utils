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

/// Deterministic work-output artifact extraction.
pub mod artifacts;
/// Where a team's build state lives.
pub mod build_store;
/// Claude Code session lineage ingestion.
pub mod claude;
/// The CLI surface: `wrkviz inspect --output <archive>`.
pub mod cli;
/// Codex rollout lineage ingestion.
pub mod codex;
/// Upstream sources compressed in place.
pub mod compressed;
/// Claude Code prompt-history ingestion.
pub mod history;
/// Site identity records.
pub mod identity;
/// The shared ingest writer and ingest commands.
pub mod ingest;
/// The `inspect` subcommand and the schema-3 archive reader it
/// is built on.
pub mod inspect;
/// The archive JSON value domain and the Python-shaped readers
/// over it.
pub mod json;
/// Muse session lineage ingestion.
pub mod muse;
/// Orc SQLite session lineage ingestion (staged port).
pub mod orc;
/// Content-addressed tool payload storage.
pub mod payloads;
/// Archive transactions: writer lock, team loading, run records.
pub mod pipeline;
/// The `query list` subcommand over schema-3 and schema-1
/// timelines.
pub mod query;
/// The seekable JSONL writer.
pub mod seekable;
/// The `serve` subcommand: a loopback static server for a
/// built archive.
pub mod serve;
/// The snapshot pointer (`snapshot-root.json`) and the legacy
/// in-archive snapshot layout.
pub mod snapshot_store;
/// The normalized team snapshot (`raw/team.json`) reader.
pub mod team_data;
/// The extract-transcripts projection.
pub mod transcript_export;
/// Archive file IO (replace-if-changed writes, JSONL).
pub mod transcript_io;
/// Calendar-date and exact-time windows.
pub mod window;
