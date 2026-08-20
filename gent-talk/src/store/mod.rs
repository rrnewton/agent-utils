//! Durable server state, behind one trait.
//!
//! Everything else in this crate is stateless: a request arrives, Discord is asked, an answer is
//! computed, nothing is kept. This module is the exception, and it is deliberately the *only*
//! exception. Two things need to outlive a process:
//!
//! * the **conversation transcript** the `/voice` page builds while the owner is talking, which
//!   today evaporates on a reload; and
//! * the **inbox state** — how far the owner has read in each channel, and which individual
//!   messages he has dealt with.
//!
//! # Read state is OURS, and it is never synchronised with Discord
//!
//! This is the decision that has to be stated once, plainly, rather than discovered later from a
//! divergence. **Discord does not share read state with bots.** There is no ack route a bot may
//! call, no read-state field on the channel object a bot can see, and no `read_state` in the
//! gateway `READY` payload for a bot user. So the read marks held here are this server's own
//! record of what the owner has been shown *by this server*:
//!
//! * **No sync-in.** Nothing here is ever populated from Discord. Marking a channel read in the
//!   Discord app has no effect on gent-talk.
//! * **No sync-back.** Marking a channel read here posts nothing, acks nothing, and changes
//!   nothing in Discord. The unread badge in the Discord app will not move.
//!
//! That holds for the per-message overlay `#50 todo-view` adds on top of it just as it holds for
//! the channel read marks: dismissing a message here is this server's record and nobody else's.
//!
//! **The store is single-tenant.** Every read mark and every dismissal is "the owner's", with no
//! column saying WHOSE — one file, one person. Sharing a deployment between two operators would
//! silently merge their inboxes, and that is not a configuration this decision survives; it would
//! have to be revisited, not worked around.
//!
//! Anything that shows a read mark to a human has to say that, because "read" is a word that
//! already means something to a Discord user and this is not that thing.
//!
//! # Why a trait, and why SQLite behind it
//!
//! The trait is as much the point as the backend. A handler that reached for `rusqlite` directly
//! would pin the deployment to one host with one filesystem forever; every call site goes through
//! [`StateStore`] so a hosted backend can be substituted without touching one of them. This is
//! the same shape [`crate::discord::DiscordClient`], [`crate::elevenlabs::SignedUrlProvider`] and
//! [`crate::retrieval::Ranker`] already have: a live implementation, plus a fake that can
//! genuinely fail.
//!
//! The shipped implementation is [`sqlite::SqliteStore`] — one file, hand-written SQL, a
//! `user_version` migration ladder. The schema is small enough that an ORM would be a dependency
//! bought with nothing, and SQLite gives the one property a file format does not: a torn write
//! rolls back rather than leaving half a record.
//!
//! # What is stored, and how it is erased
//!
//! Transcripts are the owner's own speech *and* Discord text this server read aloud, which is
//! written by third parties. It is the first thing this project retains at rest, so:
//!
//! * the database file is `0600` and its directory `0700`;
//! * retention is bounded by [`Retention`] — a conversation count, a per-conversation turn count,
//!   a cached-summary count, a dismissal count, and an age in days that applies to the first
//!   three — enforced on every write, not by a sweeper that might not run;
//! * [`StateStore::purge_everything`] erases all of it, and an operator who does not trust that
//!   can delete the single file the store lives in.

pub mod disabled;
pub mod fake;
pub mod sqlite;

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::model::{ChannelId, MessageId};

/// Longest conversation id this server will accept.
pub const MAX_ID_LEN: usize = 64;

/// Longest single turn this server will store, in characters.
///
/// A turn is one thing said out loud. This ceiling exists so a wedged client cannot turn an
/// append loop into an unbounded write; it is far above anything a person says in one breath.
pub const MAX_TURN_CHARS: usize = 8_000;

/// Milliseconds since the Unix epoch, now.
///
/// The store stamps its own records rather than trusting a client-supplied instant: the browser's
/// clock is the one thing in this system nobody controls, and a transcript ordered by it can
/// interleave wrongly for no visible reason.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Identifier of one conversation held on the `/voice` page.
///
/// It arrives from the vendor (ElevenLabs' `conversation_id`) or from the browser, so it is
/// caller-controlled text that is about to become part of a lookup key. It is validated on the
/// way in — see [`ConversationId::parse`] — and never interpolated anywhere unvalidated.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConversationId(String);

impl ConversationId {
    /// Validate caller-supplied text as a conversation id.
    ///
    /// Accepts ASCII letters, digits, `-` and `_`, up to [`MAX_ID_LEN`] characters. Everything
    /// else is refused, including the empty string. This is an allowlist rather than a
    /// denylist because the id reaches a filesystem path in some future backend even if it does
    /// not reach one today, and `../` is the obvious attack on the one after this.
    ///
    /// # Errors
    ///
    /// [`StoreError::BadId`] naming what was wrong, without echoing an unbounded amount of the
    /// caller's text back at them.
    pub fn parse(raw: &str) -> Result<Self, StoreError> {
        if raw.is_empty() {
            return Err(StoreError::BadId(
                "a conversation id must not be empty".to_owned(),
            ));
        }
        if raw.len() > MAX_ID_LEN {
            return Err(StoreError::BadId(format!(
                "a conversation id must be at most {MAX_ID_LEN} characters"
            )));
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(StoreError::BadId(
                "a conversation id may only contain letters, digits, '-' and '_'".to_owned(),
            ));
        }
        Ok(Self(raw.to_owned()))
    }

    /// Borrow the validated id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConversationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who said one turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    /// The owner, transcribed by the vendor.
    You,
    /// The voice agent.
    Agent,
    /// The page itself — a seam, a hang-up, an error it wants to keep in the record.
    Note,
}

impl Speaker {
    /// The stable text this speaker is stored as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::You => "you",
            Self::Agent => "agent",
            Self::Note => "note",
        }
    }

    /// Parse a stored speaker back.
    ///
    /// # Errors
    ///
    /// [`StoreError::Backend`] for a value this version does not know, which means the row was
    /// written by a different version and the caller must not guess at it.
    pub fn parse(raw: &str) -> Result<Self, StoreError> {
        match raw {
            "you" => Ok(Self::You),
            "agent" => Ok(Self::Agent),
            "note" => Ok(Self::Note),
            other => Err(StoreError::Backend(format!(
                "stored turn has an unknown speaker {other:?}"
            ))),
        }
    }
}

/// One thing said, in one conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// Who said it.
    pub speaker: Speaker,
    /// What was said. UNTRUSTED text when the agent is reading a channel aloud.
    pub text: String,
    /// When the store recorded it, in milliseconds since the Unix epoch. Server clock; see
    /// [`now_ms`].
    pub at_ms: i64,
}

impl Turn {
    /// A turn stamped with the current server time.
    #[must_use]
    pub fn now(speaker: Speaker, text: impl Into<String>) -> Self {
        Self {
            speaker,
            text: text.into(),
            at_ms: now_ms(),
        }
    }
}

/// One conversation, described without its contents.
///
/// This is what a listing returns. The transcript itself is a separate, explicit read, so a page
/// that only wants to say "you have three earlier conversations" does not pull every word of them
/// out of the store to find out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSummary {
    /// The conversation's id.
    pub id: ConversationId,
    /// When its first turn was recorded.
    pub started_at_ms: i64,
    /// When its most recent turn was recorded.
    pub last_at_ms: i64,
    /// How many turns it holds.
    pub turns: u32,
    /// The first line of it, condensed, so a list can be read at a glance. UNTRUSTED text.
    pub preview: String,
}

/// How far the owner has read in one channel — this server's own record.
///
/// See the module documentation: this is never read from Discord and never written back to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadMark {
    /// The channel this mark is about.
    pub channel: ChannelId,
    /// The newest message the owner has been shown. Everything after it is unread *here*.
    pub last_read: MessageId,
    /// When the mark was set, in milliseconds since the Unix epoch.
    pub marked_at_ms: i64,
}

/// What one cached summary is filed under.
///
/// Four parts, and each one answers a different way the entry can go stale:
///
/// * `version` — the whole summarisation policy, from [`crate::summarize::policy_version`].
///   Changing the prompt, the model, the width or the context window makes every old entry
///   unreachable at once, and a startup sweep collects the directories they were in.
/// * `channel` and `message` — which message it is about.
/// * `content_hash` — the message TEXT. An upstream edit changes this and nothing else, so it
///   invalidates one entry rather than the whole cache.
///
/// An upstream EDIT or DELETE orphans the old entry: it is unreachable by key and nothing
/// upstream will ever tell this server it went. The startup sweep cannot collect those — it
/// deletes by *policy version*, and an orphan is under the current version — so what collects
/// them is [`Retention`], enforced on every write of a summary: an age limit, and a ceiling on
/// how many entries exist at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryKey {
    /// The channel the message is in.
    pub channel: ChannelId,
    /// The message summarised.
    pub message: MessageId,
    /// A change detector over the message text. See [`crate::summarize::content_hash`].
    pub content_hash: u64,
    /// The summarisation policy in force when this was produced.
    pub version: String,
}

/// Bounds on how much the store keeps.
///
/// Unbounded retention is the failure this exists to prevent: a store that grows forever is both
/// a disk problem and a privacy problem, and the second one is worse. Every bound is enforced on
/// append rather than by a background sweep, so a server that is only ever started and stopped
/// still honours them.
///
/// **Every table the store has is bounded here**, not only the transcript. A cached summary is a
/// second at-rest copy of somebody else's message, and it is the ONE row a caller can cause to be
/// written without ever appending a turn — an unbounded summary table would have been the whole
/// privacy argument, quietly undone by the cache added to serve it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    /// How many conversations to keep. The oldest are dropped first. At least 1.
    pub max_conversations: u16,
    /// How many turns one conversation may hold before further appends are refused. At least 1.
    pub max_turns_per_conversation: u16,
    /// How many cached summaries to keep. The least recently written are dropped first. At
    /// least 1.
    pub max_summaries: u32,
    /// How many "dealt with" marks to keep, across all channels. The oldest are dropped first.
    ///
    /// **Deliberately NOT covered by `retain_days`, and that is the one asymmetry in this
    /// struct.** An age limit on this table would put a message the owner cleared a month ago
    /// back into his to-do list purely because time passed — a lie he cannot diagnose, since
    /// nothing on the row would say why it came back. The count bound is what stops the table
    /// growing, and it is enough here for a reason it is not enough for a summary: a dismissal
    /// holds two snowflakes and a timestamp and NO message text, so unlike a cached summary it is
    /// not a second at-rest copy of anybody's words.
    pub max_dismissals: u32,
    /// How many days a record survives: a conversation after its last turn, a cached summary
    /// after it was made. `0` means no age limit.
    ///
    /// For summaries this is also the ONLY thing that collects an orphan — an entry whose
    /// upstream message was deleted or edited away is unreachable by key and cannot be noticed
    /// any other way, because nothing tells this server that a message went.
    pub retain_days: u16,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            max_conversations: DEFAULT_MAX_CONVERSATIONS,
            max_turns_per_conversation: DEFAULT_MAX_TURNS_PER_CONVERSATION,
            max_summaries: DEFAULT_MAX_SUMMARIES,
            max_dismissals: DEFAULT_MAX_DISMISSALS,
            retain_days: DEFAULT_RETAIN_DAYS,
        }
    }
}

/// Default number of conversations kept.
pub const DEFAULT_MAX_CONVERSATIONS: u16 = 50;
/// Default number of turns one conversation may hold.
pub const DEFAULT_MAX_TURNS_PER_CONVERSATION: u16 = 1_000;
/// Default number of cached summaries kept.
///
/// Two thousand entries of a couple of hundred characters is a file measured in megabytes, which
/// is small enough not to matter and large enough that a real day's reading never evicts anything
/// it would have reused.
pub const DEFAULT_MAX_SUMMARIES: u32 = 2_000;
/// Default number of "dealt with" marks kept.
///
/// Ten thousand rows of two snowflakes and a timestamp is a few hundred kilobytes. It is set well
/// above `max_summaries` on purpose: a dismissal is the cheapest row this store has and the one
/// whose loss is most visible to the reader, because losing it puts a message he already dealt
/// with back in front of him.
pub const DEFAULT_MAX_DISMISSALS: u32 = 10_000;
/// Default age limit, in days.
pub const DEFAULT_RETAIN_DAYS: u16 = 30;

/// Why a store operation could not be carried out.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No durable store is configured, so nothing was read or written.
    ///
    /// This is a refusal, not a fallback. A server with no storage configured must not answer
    /// from an in-memory pretence that evaporates on restart — the same posture
    /// [`crate::elevenlabs::credentials`] takes toward a missing key.
    #[error("{0} is not configured; this server keeps no durable state")]
    Unavailable(&'static str),
    /// The thing asked for is not in the store.
    #[error("no such record in the store")]
    NotFound,
    /// A caller-supplied identifier was not usable.
    #[error("{0}")]
    BadId(String),
    /// A bound in [`Retention`] refuses the write.
    #[error("{0}")]
    TooLarge(String),
    /// The backend itself failed.
    #[error("the state store failed: {0}")]
    Backend(String),
}

impl StoreError {
    /// Stable machine-readable code for the API layer.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "storage_not_configured",
            Self::NotFound => "not_found",
            Self::BadId(_) => "bad_id",
            Self::TooLarge(_) => "too_large",
            Self::Backend(_) => "storage_error",
        }
    }
}

/// The durable state this server keeps.
///
/// Every method is fallible and every failure is distinguishable, because the four things that go
/// wrong here have four different fixes: nothing is configured, the record is not there, the
/// caller's identifier is wrong, or the backend broke.
#[async_trait]
pub trait StateStore: Send + Sync {
    /// A short phrase naming this backend, for the startup banner.
    fn describe(&self) -> String;

    /// Record one turn, creating the conversation if this is its first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured, [`StoreError::TooLarge`] when the
    /// conversation is already at its turn ceiling or the text exceeds [`MAX_TURN_CHARS`], and
    /// [`StoreError::Backend`] when the write fails.
    async fn append_turn(
        &self,
        conversation: &ConversationId,
        turn: &Turn,
    ) -> Result<(), StoreError>;

    /// Every stored conversation, most recently active first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on a read
    /// failure.
    async fn conversations(&self) -> Result<Vec<ConversationSummary>, StoreError>;

    /// One conversation's turns, oldest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] when the conversation is not stored, plus the errors above.
    async fn turns(&self, conversation: &ConversationId) -> Result<Vec<Turn>, StoreError>;

    /// Erase one conversation.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] when it was not there — erasing is not idempotent on purpose, so
    /// an interface can tell "erased" from "was never there" instead of claiming both.
    async fn forget_conversation(&self, conversation: &ConversationId) -> Result<(), StoreError>;

    /// Erase every conversation, returning how many were erased.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on
    /// failure.
    async fn forget_all_conversations(&self) -> Result<u64, StoreError>;

    /// This server's own read mark for one channel, if it has one.
    ///
    /// Never populated from Discord. See the module documentation.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on
    /// failure.
    async fn read_mark(&self, channel: &ChannelId) -> Result<Option<ReadMark>, StoreError>;

    /// Every read mark this server holds, in channel order.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on
    /// failure.
    async fn read_marks(&self) -> Result<Vec<ReadMark>, StoreError>;

    /// Move this server's read mark for a channel forward to `upto`.
    ///
    /// Monotonic: a mark never moves backwards, because two devices reading the same channel
    /// would otherwise flap and the older one would keep re-marking things unread. Discord is not
    /// told. See the module documentation.
    ///
    /// # Errors
    ///
    /// [`StoreError::BadId`] when `upto` is not a snowflake this server can order, plus the
    /// errors above.
    async fn mark_read(
        &self,
        channel: &ChannelId,
        upto: &MessageId,
    ) -> Result<ReadMark, StoreError>;

    /// Drop this server's read mark for a channel, making the whole window unread again.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] when there was no mark, plus the errors above.
    async fn forget_read_mark(&self, channel: &ChannelId) -> Result<(), StoreError>;

    /// Every message in this channel the owner has marked as dealt with here, newest first.
    ///
    /// The ids alone. What a caller does with them is filter a window of messages it already
    /// holds, and returning anything more would be a second copy of text this server is at pains
    /// not to keep twice.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on a read
    /// failure.
    async fn dismissals(&self, channel: &ChannelId) -> Result<Vec<MessageId>, StoreError>;

    /// Mark messages as dealt with, returning how many were not already.
    ///
    /// Idempotent by construction: dismissing something twice is not an error and does not move
    /// it, because the second tap of a control the reader cannot see the result of must not
    /// change the answer. That is also what makes the count meaningful — it is how many the
    /// reader actually cleared, which is what an undo has to restore and what a bulk action has
    /// to report.
    ///
    /// Bounded by [`Retention::max_dismissals`] on the way in, exactly as every other write here
    /// is.
    ///
    /// # Errors
    ///
    /// [`StoreError::BadId`] when an id cannot be ordered as a snowflake;
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on a
    /// write failure.
    async fn dismiss(&self, channel: &ChannelId, messages: &[MessageId])
        -> Result<u64, StoreError>;

    /// Put messages back into the to-do list, returning how many really came back.
    ///
    /// The undo, and the reason `dismiss` reports a count: restoring exactly what one action
    /// cleared is only possible if that action said what it cleared.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on a
    /// write failure.
    async fn restore(&self, channel: &ChannelId, messages: &[MessageId])
        -> Result<u64, StoreError>;

    /// The summary already produced for this exact key, if there is one.
    ///
    /// A miss is `Ok(None)`, never an error: not having summarised something yet is the normal
    /// case, not a failure.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on a read
    /// failure.
    async fn cached_summary(&self, key: &SummaryKey) -> Result<Option<String>, StoreError>;

    /// File a summary under its key, replacing any entry already there.
    ///
    /// Bounded by [`Retention`] on the way in, exactly as [`StateStore::append_turn`] is: writing
    /// one entry may evict the oldest, and evicts anything past the age limit. That is what keeps
    /// an orphaned entry — one whose message was edited or deleted upstream — from living
    /// forever. See [`SummaryKey`].
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on a
    /// write failure.
    async fn cache_summary(&self, key: &SummaryKey, summary: &str) -> Result<(), StoreError>;

    /// Delete every cached summary produced under a policy other than `version`, returning how
    /// many went.
    ///
    /// Run at startup. Without it a changed policy leaves the old entries on disk forever:
    /// unreachable, invisible, and still a copy of other people's text at rest. It collects
    /// nothing else — an entry under the CURRENT policy is never touched here, however stale it
    /// has become, which is [`Retention`]'s job.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on a
    /// write failure.
    async fn forget_summaries_except(&self, version: &str) -> Result<u64, StoreError>;

    /// Erase everything this store holds.
    ///
    /// This is the operator's purge. It must leave the store usable, not deleted, so a running
    /// server keeps working afterwards.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unavailable`] when no store is configured; [`StoreError::Backend`] on
    /// failure.
    async fn purge_everything(&self) -> Result<(), StoreError>;
}

/// The standing statement that read state here is not Discord's.
///
/// It rides along on every inbox answer for the same reason
/// [`crate::untrusted::NOTICE`] rides along on every read: the alternative is that the owner
/// discovers it from a divergence — an unread badge in the Discord app that will not clear, or a
/// channel gent-talk calls unread that he read on his laptop an hour ago — and has to guess which
/// of the two is broken. Neither is. They are different records, and only one of them is ours.
pub const INBOX_NOTICE: &str = "Read state is gent-talk's own. Discord shares none with a bot, so \
                                nothing here is read from Discord and nothing here is written \
                                back to it: marking a channel read here does not clear its badge \
                                in the Discord app, and clearing it there does not change this.";

/// Condense one turn into a listing preview.
///
/// Shared by every backend so two of them cannot disagree about what a listing shows.
#[must_use]
pub fn preview_of(text: &str) -> String {
    crate::summary::condense(text, PREVIEW_CHARS)
}

/// Width of a conversation preview line.
pub const PREVIEW_CHARS: usize = 120;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_id_refuses_path_traversal_and_everything_like_it() {
        for hostile in [
            "../etc/passwd",
            "a/b",
            "a\\b",
            "a b",
            "a.b",
            "",
            "conv\0",
            "café",
        ] {
            let error = ConversationId::parse(hostile)
                .expect_err(&format!("{hostile:?} must not be accepted as an id"));
            assert_eq!(error.code(), "bad_id", "{hostile:?}: {error}");
        }
        assert_eq!(
            ConversationId::parse("conv_01-ABC")
                .expect("an ordinary id is fine")
                .as_str(),
            "conv_01-ABC"
        );
    }

    #[test]
    fn a_conversation_id_is_length_capped() {
        let long = "a".repeat(MAX_ID_LEN);
        assert!(ConversationId::parse(&long).is_ok());
        let too_long = "a".repeat(MAX_ID_LEN + 1);
        assert!(ConversationId::parse(&too_long).is_err());
    }

    #[test]
    fn the_error_codes_separate_the_four_different_fixes() {
        assert_eq!(
            StoreError::Unavailable("storage.path").code(),
            "storage_not_configured"
        );
        assert_eq!(StoreError::NotFound.code(), "not_found");
        assert_eq!(StoreError::BadId("no".to_owned()).code(), "bad_id");
        assert_eq!(StoreError::TooLarge("no".to_owned()).code(), "too_large");
        assert_eq!(StoreError::Backend("no".to_owned()).code(), "storage_error");
    }

    #[test]
    fn a_speaker_round_trips_and_an_unknown_one_is_refused_rather_than_guessed() {
        for speaker in [Speaker::You, Speaker::Agent, Speaker::Note] {
            assert_eq!(
                Speaker::parse(speaker.as_str()).expect("round trip"),
                speaker
            );
        }
        let error = Speaker::parse("system").expect_err("must refuse");
        assert_eq!(error.code(), "storage_error");
    }
}
