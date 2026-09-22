//! Read-only replay and disposable indexing for append-only slot event logs.
//!
//! This crate never writes the event directory. Its only mutable output is a
//! derived SQLite index that can be discarded and rebuilt from the event log.
//! Event numbers are accepted only when they have an exact `i64`/`u64`
//! representation or decode to a finite `f64`; within that domain hashes use
//! the Python authority's canonical spelling. Wider integers and non-finite
//! values are refused instead of being rounded into a different hash. Event
//! documents are limited to 16 MiB and 127 nested JSON containers, and strings
//! must decode to Unicode scalar values (so lone surrogate escapes are
//! refused). Replay also refuses unknown event kinds, non-array imported holds,
//! and non-object state evidence even where the current Python reader does not
//! inspect those values; the Python writer always emits the stricter form.
//!
//! Replay validates the complete intrinsic active/archive record schema, nested
//! identities, checkout identities, historical imports, revisions, and recovery
//! evidence. It deliberately has no configuration input, so checks that compare
//! checkout paths, repository paths, layout defaults, or landed refs with the
//! originating registry configuration remain the Python authority's job. This
//! is the path-independent portion of its `require_repository = false` replay,
//! not a replacement for configuration-coupled policy validation.
//!
//! `wrkslotsd` is an unpublished migration component with only `rebuild` and
//! `status`; operators should continue to use the parent `wrkslots quickstart`
//! and `wrkslots --userguide` documentation until it becomes a public command.

#![forbid(unsafe_code)]

mod canonical;
pub mod cli;
mod index;
mod replay;
mod schema;

use std::error::Error;
use std::fmt;

pub use canonical::canonical_sha256;
pub use index::{read_index, rebuild_index};
pub use replay::{replay, ReplaySummary};

/// An invalid event log, inaccessible file, or unusable derived index.
#[derive(Debug)]
pub struct ObserverError {
    message: String,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl ObserverError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn with_source(
        message: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for ObserverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ObserverError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests;
