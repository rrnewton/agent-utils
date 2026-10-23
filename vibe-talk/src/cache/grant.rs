//! Who may write to the message cache: a [`FillGrant`]. `#208 persistent-cache`.
//!
//! Whatever an upstream read sees may be offered to the cache only under a grant, and a grant
//! comes from exactly two places:
//!
//! - [`FillGrant::for_caller`], for a read a request asked for. It answers `None` for a read-scope
//!   caller, so nothing such a caller sees is ever written durably: no rows, no coverage, no
//!   tombstones, no last-used times.
//! - [`FillGrant::server`], for a read the server makes on its own account, named by
//!   [`ServerFill`].
//!
//! The grant's one field, and its type, are private and there is no other constructor, so code
//! outside this module cannot make one up:
//!
//! ```
//! use vibe_talk::auth::Scope;
//! use vibe_talk::cache::grant::{FillGrant, ServerFill};
//!
//! assert!(FillGrant::for_caller(Scope::Read).is_none());
//! assert!(FillGrant::for_caller(Scope::Write).is_some());
//! assert_eq!(FillGrant::server(ServerFill::Poller).label(), "poller");
//! ```
//!
//! ```compile_fail
//! use vibe_talk::cache::grant::FillGrant;
//!
//! // Refused: the field, and the type it holds, are private to this module.
//! let _forged = FillGrant { authority: unreachable!() };
//! ```
//!
//! ```compile_fail
//! use vibe_talk::cache::grant::FillGrant;
//!
//! // Refused: there is no default grant either.
//! let _forged = FillGrant::default();
//! ```

use crate::auth::Scope;

/// The authority under which a read's observations may be written to the message cache.
///
/// Minted only by [`FillGrant::for_caller`] (write scope and above) and [`FillGrant::server`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillGrant {
    authority: Authority,
}

/// Whose read a grant covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Authority {
    /// A request from a caller with write scope or above.
    Caller,
    /// The server, reading on its own account.
    Server(ServerFill),
}

/// The reads the server makes on its own account, each of which may fill the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServerFill {
    /// The live poller's reads of a polled channel.
    Poller,
    /// Changes an authenticated provider adapter pushed to the ingest routes.
    Ingest,
    /// New messages a provider's own event stream delivered with their text (`#452
    /// slack-live-ingest`). Offered as an adapter's push, whose text a later read proves whole,
    /// but never as a feed that vouches for what it did not report: the stream carries no edits,
    /// deletions or other bots' posts.
    Stream,
    /// The messages this server posted itself.
    OwnPost,
    /// A scheduled read that keeps spans already held current.
    Refresher,
    /// A walk into a channel's older history that the server starts itself.
    Backfill,
    /// The reads a voice session starts ahead of its agent's first question.
    VoiceWarmup,
}

impl ServerFill {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 7] = [
        Self::Poller,
        Self::Ingest,
        Self::Stream,
        Self::OwnPost,
        Self::Refresher,
        Self::Backfill,
        Self::VoiceWarmup,
    ];

    /// A short, stable name for logs and counters: lowercase words joined by hyphens.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Poller => "poller",
            Self::Ingest => "ingest",
            Self::Stream => "stream",
            Self::OwnPost => "own-post",
            Self::Refresher => "refresher",
            Self::Backfill => "backfill",
            Self::VoiceWarmup => "voice-warmup",
        }
    }
}

impl FillGrant {
    /// The grant a request's caller holds: `Some` for write scope and above, `None` for read
    /// scope, whose reads write nothing durable.
    ///
    /// A match, not a comparison, so a scope added later has to be decided here explicitly.
    #[must_use]
    pub fn for_caller(scope: Scope) -> Option<Self> {
        match scope {
            Scope::Read => None,
            Scope::Write => Some(Self {
                authority: Authority::Caller,
            }),
        }
    }

    /// The grant for a read the server makes on its own account.
    #[must_use]
    pub fn server(kind: ServerFill) -> Self {
        Self {
            authority: Authority::Server(kind),
        }
    }

    /// Which server read this grant is for; `None` for a caller's.
    #[must_use]
    pub fn server_fill(&self) -> Option<ServerFill> {
        match self.authority {
            Authority::Caller => None,
            Authority::Server(kind) => Some(kind),
        }
    }

    /// A short, stable name for logs and counters: `caller`, or [`ServerFill::label`].
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self.authority {
            Authority::Caller => "caller",
            Authority::Server(kind) => kind.label(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn a_read_scope_caller_gets_no_grant() {
        assert_eq!(FillGrant::for_caller(Scope::Read), None);
    }

    #[test]
    fn a_write_scope_caller_gets_a_caller_grant() {
        let grant = FillGrant::for_caller(Scope::Write).expect("write scope may fill");
        assert_eq!(grant.label(), "caller");
        assert_eq!(grant.server_fill(), None);
    }

    #[test]
    fn every_server_kind_is_labelled_and_carried_by_its_grant() {
        let mut labels = HashSet::new();
        for kind in ServerFill::ALL {
            let label = kind.label();
            assert!(
                !label.is_empty()
                    && label.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                    && !label.starts_with('-')
                    && !label.ends_with('-'),
                "{kind:?} has the label {label:?}"
            );
            assert!(labels.insert(label), "{label:?} is used twice");
            assert_ne!(label, "caller", "a server kind must not read as a caller");
            let grant = FillGrant::server(kind);
            assert_eq!(grant.label(), label);
            assert_eq!(grant.server_fill(), Some(kind));
        }
        assert_eq!(
            labels,
            HashSet::from([
                "poller",
                "ingest",
                "stream",
                "own-post",
                "refresher",
                "backfill",
                "voice-warmup",
            ])
        );
    }
}
