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
        f.write_str(&self.format_for(self.provider))
    }
}

impl RateLimitExhausted {
    fn format_for(&self, provider: &str) -> String {
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
        format!(
            "{provider} RATE LIMIT ({scope}) on {route}: {gave_up}; waited {waited:.1}s of a \
             {budget:.0}s budget and {provider} still wants {retry_after:.1}s more",
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
    #[error("chat request failed: {0}")]
    Transport(String),
    /// The provider answered with a non-success status.
    ///
    /// **Not 429.** A rate limit is [`ChatError::RateLimited`] and only ever appears after the
    /// client has waited it out and failed; a bare 429 never reaches a caller.
    #[error("chat returned HTTP {status}: {body}")]
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
    #[error("chat response could not be understood: {0}")]
    Shape(String),
    /// The server refused the operation before contacting the provider.
    #[error("refused: {0}")]
    Refused(String),
    /// A failure labeled by the backend that actually handled the request.
    #[error("{}", source.format_for(provider))]
    Provider {
        /// Human-readable name supplied by [`ChatClient::provider_name`].
        provider: String,
        /// Original typed failure, retained for classification and retry handling.
        #[source]
        source: Box<ChatError>,
    },
}

impl ChatError {
    /// Attach the backend's display name without losing the typed failure.
    #[must_use]
    pub fn with_provider(self, provider: &str) -> Self {
        Self::Provider {
            provider: provider.to_owned(),
            source: Box::new(self),
        }
    }

    /// The underlying failure, independent of its provider label.
    #[must_use]
    pub fn cause(&self) -> &Self {
        match self {
            Self::Provider { source, .. } => source.cause(),
            other => other,
        }
    }

    fn format_for(&self, provider: &str) -> String {
        match self.cause() {
            Self::Transport(detail) => format!("{provider} request failed: {detail}"),
            Self::Status { status, body } => format!("{provider} returned HTTP {status}: {body}"),
            Self::Shape(detail) => format!("{provider} response could not be understood: {detail}"),
            Self::Refused(detail) => format!("{provider} refused: {detail}"),
            Self::RateLimited(detail) => detail.format_for(provider),
            Self::Provider { .. } => unreachable!("cause removes provider context"),
        }
    }

    /// How long the provider still wants us to wait, when this failure is a rate limit.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self.cause() {
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

/// A channel created or resolved by a provider-side registration bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredChannel {
    /// Stable channel id used by all subsequent vibe-talk routes.
    pub id: ChannelId,
    /// Whether this call created the upstream registration rather than finding an existing one.
    pub created: bool,
    /// Whether the provider bridge permits posting into this channel.
    ///
    /// Registration responses that omit this field remain read-only for backward compatibility.
    pub writable: bool,
}

/// Read and post access to configured chat channels.
#[async_trait]
pub trait ChatClient: Send + Sync {
    /// Human-readable name of the source chat service, including when accessed through a bridge.
    fn provider_name(&self) -> &str {
        "Chat"
    }

    /// Whether this backend supports channel and thread timeline views.
    fn supports_threading(&self) -> bool {
        false
    }

    /// Read one backward page of a channel view, with backend-owned opaque cursors.
    ///
    /// The backend must verify thread membership before any thread-specific read. Discovery that
    /// cannot be completed must fail rather than returning an apparently complete flat history.
    async fn fetch_timeline(
        &self,
        _channel: &ChannelId,
        _request: &crate::threads::TimelineRequest,
    ) -> Result<crate::threads::TimelinePage, ChatError> {
        Err(ChatError::Refused(
            "thread timelines are not supported by this backend".to_owned(),
        ))
    }

    /// Post inside a thread after verifying that it belongs to the configured parent channel.
    ///
    /// The caller also enforces the parent channel's write allowlist. A reply-to message does not
    /// implicitly select a thread: the destination is always explicit.
    async fn post_in_thread(
        &self,
        _channel: &ChannelId,
        _thread_id: &str,
        _content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused(
            "thread posting is not supported by this backend".to_owned(),
        ))
    }

    /// Fetch an exact message after verifying its thread belongs to the configured channel.
    async fn fetch_thread_message(
        &self,
        _channel: &ChannelId,
        _thread_id: &str,
        _message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused(
            "thread message lookup is not supported by this backend".to_owned(),
        ))
    }

    /// Whether this provider accepts operator-supplied channel references and manages them upstream.
    ///
    /// False by default so direct providers and existing test doubles keep the established
    /// `{id, label, writable}` registration flow.
    fn supports_channel_registration(&self) -> bool {
        false
    }

    /// Register an operator-supplied channel reference and return its stable local id.
    ///
    /// # Errors
    ///
    /// Returns [`ChatError`] when registration is disabled, rejected, or fails.
    async fn register_channel(
        &self,
        _source: &str,
        _label: &str,
    ) -> Result<RegisteredChannel, ChatError> {
        Err(ChatError::Refused(
            "the configured chat provider does not support channel registration".to_owned(),
        ))
    }

    /// Remove a channel previously registered through the provider bridge.
    ///
    /// # Errors
    ///
    /// Returns [`ChatError`] when registration is disabled, rejected, or fails.
    async fn unregister_channel(&self, _channel: &ChannelId) -> Result<(), ChatError> {
        Err(ChatError::Refused(
            "the configured chat provider does not support channel registration".to_owned(),
        ))
    }

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

    /// Read a minimal sample for channel reachability checks.
    ///
    /// Backends with thread-only containers can probe their root-message view instead of calling
    /// a main-message endpoint that the provider does not support for those containers.
    async fn probe_channel(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.fetch_recent(channel, limit).await
    }

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
