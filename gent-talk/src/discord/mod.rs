//! Discord access, behind a trait so the rest of the server is testable without a live Discord.
//!
//! v0 needs exactly two operations: read the recent messages of a channel, and post one message
//! back. Both are pulled on demand — this server has no gateway connection and no webhook
//! receiver, so it never needs to be running for a message to survive.

pub mod fake;
pub mod http;
pub mod ratelimit;

use std::time::Duration;

use async_trait::async_trait;

use crate::model::{ChannelId, Message, MessageId};

/// A Discord read/post failure.
#[derive(Debug, thiserror::Error)]
pub enum DiscordError {
    /// The request never completed.
    #[error("discord request failed: {0}")]
    Transport(String),
    /// Discord answered with a non-success status.
    ///
    /// **Not 429.** A rate limit is [`DiscordError::RateLimited`] and only ever after the client
    /// has waited it out and failed; a bare 429 never reaches a caller.
    #[error("discord returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated by the caller.
        body: String,
    },
    /// Discord rate-limited the request and the wait did not clear inside the client's budget.
    ///
    /// The client obeys `Retry-After` — see [`ratelimit`] — so this is the *exhausted* case, and
    /// it is deliberately its own variant rather than an HTTP 429 in [`DiscordError::Status`]. A
    /// caller that can slow itself down needs to be able to tell "we are over quota, and here is
    /// how long is left" from "Discord answered 500", and its [`Display`](std::fmt::Display) names
    /// the rate limit so a log line does too.
    #[error("{0}")]
    RateLimited(ratelimit::RateLimitExhausted),
    /// The response did not have the shape this server expects.
    #[error("discord response could not be understood: {0}")]
    Shape(String),
    /// The server refused the operation before contacting Discord.
    #[error("refused: {0}")]
    Refused(String),
}

impl DiscordError {
    /// How long Discord still wants us to wait, when this failure is a rate limit.
    ///
    /// This is the seam that keeps the two notions of "slow down" in this crate from being two:
    /// [`crate::live`]'s per-channel backoff consults it, so the poll loop's wait can be longer
    /// than Discord asked for but never shorter. `None` for every other failure, which is the
    /// honest answer — a 500 carries no instruction about when to come back.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited(detail) => Some(detail.retry_after),
            _ => None,
        }
    }
}

/// Read and post access to Discord channels.
#[async_trait]
pub trait DiscordClient: Send + Sync {
    /// Fetch one page of a channel, returned OLDEST FIRST.
    ///
    /// Discord itself returns newest-first; implementations normalize so that everything above
    /// this trait reads in conversation order.
    ///
    /// `before` and `after` are Discord's own cursors and are **mutually exclusive** — Discord
    /// does not define what sending both means, so an implementation must refuse rather than pick.
    /// Their asymmetry matters and is easy to get backwards: `before` walks BACKWARDS and yields
    /// the newest messages older than the cursor, whereas `after` yields the OLDEST messages newer
    /// than it. Inverting `after` produces a walk that looks right against a fake and skips
    /// messages against live Discord.
    ///
    /// This is the required method. [`DiscordClient::fetch_recent`] is a thin default over it, so
    /// a client cannot implement the uncursored case and silently ignore the cursor.
    ///
    /// # Errors
    ///
    /// Returns [`DiscordError`] when the request fails, is rejected, or cannot be parsed, and
    /// [`DiscordError::Refused`] when both cursors are supplied.
    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, DiscordError>;

    /// Fetch up to `limit` most recent messages, returned OLDEST FIRST.
    ///
    /// # Errors
    ///
    /// As [`DiscordClient::fetch_page`].
    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, DiscordError> {
        self.fetch_page(channel, limit, None, None).await
    }

    /// Post `content` to `channel`, optionally as a reply to an existing message.
    ///
    /// # Errors
    ///
    /// Returns [`DiscordError`] when the post fails or is rejected.
    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, DiscordError>;
}
