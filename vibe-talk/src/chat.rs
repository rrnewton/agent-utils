//! Provider-neutral access to the chat service behind the application.
//!
//! Provider implementations translate their wire format into [`crate::model`] types. The rest of
//! the server depends on this module, not on a particular provider. The paging cursor is still a
//! [`MessageId`] for compatibility with the existing Discord API; making cursors fully opaque is a
//! separate change because it also changes ordering and durable-state semantics.

use std::time::Duration;

use async_trait::async_trait;

use crate::model::{ChannelId, Message, MessageId};

/// Why a provider request exhausted its bounded rate-limit wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateLimitExhausted {
    /// Stable lowercase provider name used in the diagnostic.
    pub provider: &'static str,
    /// The method and path whose bucket was exhausted.
    pub route: String,
    /// How many times the request was actually sent.
    pub attempts: u32,
    /// How long this call spent waiting before giving up.
    pub waited: Duration,
    /// The total-wait budget it was measured against.
    pub budget: Duration,
    /// What the provider last asked for, and which was not affordable.
    pub retry_after: Duration,
    /// Whether the limit was global rather than per-route.
    pub global: bool,
}

impl std::fmt::Display for RateLimitExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scope = if self.global {
            "GLOBAL, the whole bot token"
        } else {
            "this route's bucket"
        };
        let gave_up = if self.attempts == 0 {
            "gave up without sending it, because that bucket is known to be empty".to_owned()
        } else {
            format!("gave up after {} attempt(s)", self.attempts)
        };
        write!(
            f,
            "{provider} RATE LIMIT ({scope}) on {route}: {gave_up}; waited {waited:.1}s of a \
             {budget:.0}s budget and {provider} still wants {retry_after:.1}s more",
            provider = self.provider,
            route = self.route,
            waited = self.waited.as_secs_f64(),
            budget = self.budget.as_secs_f64(),
            retry_after = self.retry_after.as_secs_f64(),
        )
    }
}

/// A chat-provider read or post failure.
#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    /// The request never completed.
    // Keep the established wording while Discord is the only selectable provider. A future
    // provider-aware configuration can carry the provider name without changing this enum again.
    #[error("discord request failed: {0}")]
    Transport(String),
    /// The provider answered with a non-success status.
    ///
    /// **Not 429.** A rate limit is [`ChatError::RateLimited`] and only ever appears after the
    /// client has waited it out and failed; a bare 429 never reaches a caller.
    #[error("discord returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated by the caller.
        body: String,
    },
    /// The provider rate-limited the request and the wait did not clear inside the client's budget.
    ///
    #[error("{0}")]
    RateLimited(RateLimitExhausted),
    /// The response did not have the shape this server expects.
    #[error("discord response could not be understood: {0}")]
    Shape(String),
    /// The server refused the operation before contacting the provider.
    #[error("refused: {0}")]
    Refused(String),
}

impl ChatError {
    /// How long the provider still wants us to wait, when this failure is a rate limit.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited(detail) => Some(detail.retry_after),
            _ => None,
        }
    }
}

/// The account a chat backend posts as, as reported by the provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatIdentity {
    /// Provider-stable account identifier.
    pub id: String,
    /// Human-readable account name.
    pub username: String,
}

/// Read and post access to configured chat channels.
#[async_trait]
pub trait ChatClient: Send + Sync {
    /// Whether this provider can move its own read cursor forward.
    ///
    /// False by default so existing providers and test doubles do not acquire a write capability
    /// merely by upgrading vibe-talk.
    fn supports_upstream_read_mark(&self) -> bool {
        false
    }

    /// Ask the provider which account the configured credential belongs to.
    ///
    /// # Errors
    ///
    /// Returns [`ChatError`] when the provider cannot authenticate or describe the account.
    async fn identity(&self) -> Result<ChatIdentity, ChatError>;

    /// Fetch one page of a channel, returned oldest first.
    ///
    /// `before` walks backward and `after` walks forward. They are mutually exclusive. These are
    /// message-id cursors for compatibility with Discord; the future opaque-cursor layer belongs
    /// here rather than in callers.
    ///
    /// # Errors
    ///
    /// Returns [`ChatError`] when the request fails, is rejected, or cannot be parsed.
    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError>;

    /// Fetch up to `limit` most recent messages, returned oldest first.
    ///
    /// # Errors
    ///
    /// As [`ChatClient::fetch_page`].
    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.fetch_page(channel, limit, None, None).await
    }

    /// Post `content` to `channel`, optionally as a reply to an existing message.
    ///
    /// # Errors
    ///
    /// Returns [`ChatError`] when the post fails or is rejected.
    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError>;

    /// Move the provider's read cursor through one message.
    ///
    /// Providers define how threads participate in that cursor. This is deliberately separate
    /// from vibe-talk's local read mark and its reversible Done/archive state.
    async fn mark_read_upstream(
        &self,
        _channel: &ChannelId,
        _through: &MessageId,
    ) -> Result<(), ChatError> {
        Err(ChatError::Refused(
            "the configured chat provider does not support upstream read marks".to_owned(),
        ))
    }
}
