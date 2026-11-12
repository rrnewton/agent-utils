//! Rust CI hub ledger core: typed events, one fold, one predicate, one digest.
//!
//! The crate is storage-agnostic. [`pyjson`] carries Python-compatible JSON
//! values so byte identity is decidable, [`event`] defines the append-only
//! vocabulary with its refusing union, [`expand`] restores stored codec
//! lines to logical events, [`fold`] folds admitted events into terminal
//! rows, [`digest`] owns the single canonical serializers, and [`verdict`]
//! owns the single qualifying predicate. Storage connectors, the disposable
//! index, and the CLI build on this core without re-implementing any of
//! those.

pub mod digest;
pub mod event;
pub mod expand;
pub mod fold;
pub mod pyjson;
pub mod verdict;
