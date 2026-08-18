//! Discord access, behind a trait so the rest of the server is testable without a live Discord.
//!
//! v0 needs exactly two operations: read the recent messages of a channel, and post one message
//! back. Both are pulled on demand — this server has no gateway connection and no webhook
//! receiver, so it never needs to be running for a message to survive.

pub mod fake;
pub mod http;

use async_trait::async_trait;

use crate::model::{ChannelId, Message, MessageId};

/// A Discord read/post failure.
#[derive(Debug, thiserror::Error)]
pub enum DiscordError {
    /// The request never completed.
    #[error("discord request failed: {0}")]
    Transport(String),
    /// Discord answered with a non-success status.
    #[error("discord returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated by the caller.
        body: String,
    },
    /// The response did not have the shape this server expects.
    #[error("discord response could not be understood: {0}")]
    Shape(String),
    /// The server refused the operation before contacting Discord.
    #[error("refused: {0}")]
    Refused(String),
}

/// Read and post access to Discord channels.
#[async_trait]
pub trait DiscordClient: Send + Sync {
    /// Fetch up to `limit` most recent messages, returned OLDEST FIRST.
    ///
    /// Discord itself returns newest-first; implementations normalize so that everything above
    /// this trait reads in conversation order.
    ///
    /// # Errors
    ///
    /// Returns [`DiscordError`] when the request fails, is rejected, or cannot be parsed.
    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, DiscordError>;

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
