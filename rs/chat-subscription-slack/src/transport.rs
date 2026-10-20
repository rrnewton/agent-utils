//! The small interface between the Slack provider and whatever actually reaches Slack.
//!
//! Inbound, a transport opens one ordered event stream for the configured channels, starting at a
//! [`Catchup`] position. Each sequenced record carries the provider's durable event sequence
//! number, which the plugin uses as its replay cursor. Outbound, a transport posts a message, adds
//! a reaction, and lists recent messages so an uncertain post can be reconciled. Directory reads
//! resolve a sender query to one user ID and enumerate a conversation's current members.
//!
//! Every write takes an [`AppId`]: there is no way to ask a transport to post as a user.

use std::sync::Arc;
use std::time::Duration;

use crate::ids::AppId;

/// Where a new event stream starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Catchup {
    /// Only events that occur after the stream starts.
    None,
    /// Replay the provider's retained history first.
    Oldest,
    /// Replay from this durable event sequence number.
    Sequence(u64),
    /// Replay events from the last this-many seconds first.
    SinceSeconds(u64),
}

/// What to subscribe to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscribeSpec {
    /// Slack conversation IDs.
    pub channel_ids: Vec<String>,
    /// Starting position.
    pub catchup: Catchup,
}

/// The Slack fields of one `message.created` event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlackMessage {
    /// Conversation ID.
    pub channel_id: String,
    /// The message's own `ts`.
    pub message_ts: String,
    /// The thread root's `ts` for a thread reply (or a thread root that has replies).
    pub thread_ts: Option<String>,
    /// Slack user ID of the author.
    pub user_id: Option<String>,
    /// Bot annotation, which can also accompany a person's user-token post.
    pub bot_id: Option<String>,
    /// Delivery app annotation; this does not identify the author.
    pub app_id: Option<String>,
    /// Slack message subtype; `None` for an ordinary message.
    pub subtype: Option<String>,
    /// Message text as Slack stores it (with `&amp;`, `&lt;`, `&gt;` escapes).
    pub text: String,
    /// Workspace ID, when reported.
    pub team_id: Option<String>,
}

/// One classified record from an event stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamRecord {
    /// A sequenced event.
    Event {
        /// Durable event sequence number, the replay cursor.
        sequence: u64,
        /// Event type, for example `message.created`.
        event_type: String,
        /// The Slack message for a `message.created` event that carried one.
        message: Option<Box<SlackMessage>>,
    },
    /// The subscription refreshed.
    Heartbeat,
    /// The provider reported lost events.
    Gap(String),
    /// Informational output, logged and otherwise ignored.
    Notice(String),
}

/// The result of waiting on a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamWait {
    /// One record arrived.
    Record(StreamRecord),
    /// Nothing arrived within the wait.
    Idle,
    /// The stream ended; the reason is a bounded diagnostic.
    Closed(String),
    /// [`StreamCancel::cancel`] was called.
    Cancelled,
}

/// Cancellation usable from another thread while [`EventStream::next`] blocks.
pub trait StreamCancel: Send + Sync {
    /// Make the current and every later [`EventStream::next`] return [`StreamWait::Cancelled`].
    fn cancel(&self);
}

/// One open, ordered event stream.
pub trait EventStream: Send {
    /// Wait up to `wait` for the next record.
    fn next(&mut self, wait: Duration) -> StreamWait;
    /// Cancellation authority for a blocked [`Self::next`].
    fn canceller(&self) -> Arc<dyn StreamCancel>;
    /// Stop the stream and release its resources.
    ///
    /// # Errors
    ///
    /// Returns a bounded diagnostic when cleanup could not be confirmed.
    fn close(&mut self) -> Result<(), String>;
}

/// How a post's body is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFormat {
    /// Standard Markdown, rendered by Slack's client.
    Markdown,
    /// Plain text, shown as typed.
    Text,
}

/// One post request.
#[derive(Clone, Copy, Debug)]
pub struct PostMessage<'a> {
    /// The app the post acts as. Required: a post never uses a user identity.
    pub app_id: &'a AppId,
    /// Conversation ID.
    pub channel_id: &'a str,
    /// Thread root to reply in; `None` posts a new top-level message.
    pub thread_ts: Option<&'a str>,
    /// Message body.
    pub text: &'a str,
    /// Body format.
    pub format: TextFormat,
}

/// One message from a history or thread listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedMessage {
    /// Message `ts`.
    pub ts: String,
    /// Thread root `ts`, when in a thread.
    pub thread_ts: Option<String>,
    /// Author user ID, when reported.
    pub author_user_id: Option<String>,
    /// Whether a bot or app wrote the message.
    pub is_author_bot: bool,
    /// Message content as plain text.
    pub content: String,
}

/// A listing request used to reconcile an uncertain post.
#[derive(Clone, Copy, Debug)]
pub struct ListQuery<'a> {
    /// The app the read acts as.
    pub app_id: &'a AppId,
    /// Conversation ID.
    pub channel_id: &'a str,
    /// Thread root to list; `None` lists the conversation's top-level history.
    pub thread_ts: Option<&'a str>,
    /// Only messages after this Slack timestamp.
    pub oldest: &'a str,
}

/// Whether a failed write is proven not to have happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Slack answered with an error: the write did not happen.
    NotApplied,
    /// The write may or may not have happened.
    Unknown,
}

/// A bounded transport failure; it never contains message text or credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError {
    /// Lowercase slug, for example `ratelimited` or `transport_timeout`.
    pub code: String,
    /// Bounded diagnostic.
    pub detail: String,
    /// Application evidence.
    pub outcome: Outcome,
    /// Whether repeating the same operation may succeed.
    pub retryable: bool,
}

impl TransportError {
    /// A failure proven not to have applied.
    #[must_use]
    pub fn not_applied(code: &str, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: slug(code),
            detail: bounded(&detail.into()),
            outcome: Outcome::NotApplied,
            retryable,
        }
    }

    /// A failure whose effect is unknown.
    #[must_use]
    pub fn unknown(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: slug(code),
            detail: bounded(&detail.into()),
            outcome: Outcome::Unknown,
            retryable: true,
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: {} (outcome={:?}, retryable={})",
            self.code, self.detail, self.outcome, self.retryable
        )
    }
}

/// Reduce an arbitrary code to the bridge's lowercase slug alphabet.
#[must_use]
pub fn slug(code: &str) -> String {
    let mut value: String = code
        .chars()
        .map(|character| {
            let lower = character.to_ascii_lowercase();
            if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
                lower
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    if value.is_empty() {
        value.push_str("transport_error");
    }
    value
}

/// Bound a diagnostic to 2,000 bytes on a character boundary, never empty.
#[must_use]
pub fn bounded(detail: &str) -> String {
    let flattened = detail.replace(['\n', '\r', '\0'], " ");
    let trimmed = flattened.trim();
    if trimmed.is_empty() {
        return "no detail".to_owned();
    }
    let mut boundary = trimmed.len().min(2_000);
    while !trimmed.is_char_boundary(boundary) {
        boundary -= 1;
    }
    trimmed[..boundary].to_owned()
}

/// Everything the provider needs from Slack.
pub trait SlackTransport: Send {
    /// Open one ordered event stream.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the stream cannot be started.
    fn open_stream(&mut self, spec: &SubscribeSpec)
        -> Result<Box<dyn EventStream>, TransportError>;

    /// Post one message as the given app and return its `ts`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`]; [`Outcome::Unknown`] when the post may have happened.
    fn post_message(&mut self, post: &PostMessage<'_>) -> Result<String, TransportError>;

    /// Add a reaction as the given app. Returns `true` when it was already present.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`].
    fn add_reaction(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError>;

    /// List recent messages, oldest bound inclusive of nothing before `oldest`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`].
    fn list_messages(
        &mut self,
        query: &ListQuery<'_>,
    ) -> Result<Vec<ListedMessage>, TransportError>;

    /// Resolve one unixname, email address or Slack user ID to its provider user ID.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the query cannot be resolved unambiguously. Transports
    /// without a directory refuse; callers can accept a raw Slack user ID instead.
    fn resolve_sender(&mut self, _app_id: &AppId, _query: &str) -> Result<String, TransportError> {
        Err(TransportError::not_applied(
            "directory_unsupported",
            "this transport cannot resolve people; pass a Slack user ID (U… or W…) from the person's Slack profile",
            false,
        ))
    }

    /// Enumerate every current member of one conversation as an explicit Slack user ID.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] if enumeration is incomplete or unsupported. A partial list
    /// must never be returned as a successful result.
    fn list_members(
        &mut self,
        _app_id: &AppId,
        _channel_id: &str,
    ) -> Result<Vec<String>, TransportError> {
        Err(TransportError::not_applied(
            "directory_unsupported",
            "this transport cannot enumerate channel members; pass explicit Slack user IDs instead",
            false,
        ))
    }

    /// Check authoritative account data for this exact sender. Only an explicit human result
    /// permits a rejection reaction; absent or ambiguous account information does not.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the account cannot be checked. Unsupported transports
    /// refuse without applying a reaction.
    fn sender_is_human(
        &mut self,
        _app_id: &AppId,
        _sender_id: &str,
    ) -> Result<bool, TransportError> {
        Err(TransportError::not_applied(
            "author_not_human",
            "this transport cannot verify that the sender is a human; no rejection reaction was applied",
            false,
        ))
    }

    /// Add a rejection reaction only after authoritative account data confirms a human sender.
    /// Implementations with several client calls must share one deadline across the operation.
    /// After the account check, call `before_write` successfully before any possible reaction;
    /// its persistence time is part of that same deadline. A failed account check must not
    /// call it. The helper uses it to durably bind the operation's first reaction choice.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] before any write for a nonhuman or unverified author, or when
    /// checking the account or adding the reaction fails.
    fn add_rejection(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
        message_ts: &str,
        sender_id: &str,
        name: &str,
        before_write: &mut dyn FnMut() -> Result<(), TransportError>,
    ) -> Result<bool, TransportError> {
        if !self.sender_is_human(app_id, sender_id)? {
            return Err(unverified_rejection_author());
        }
        before_write()?;
        self.add_reaction(app_id, channel_id, message_ts, name)
    }
}

pub(crate) fn unverified_rejection_author() -> TransportError {
    TransportError::not_applied(
        "author_not_human",
        "authoritative account data did not confirm this exact sender is human; no rejection reaction was applied",
        false,
    )
}

/// Placeholder for a direct Slack Web API transport (`conversations.history`,
/// `conversations.replies`, `chat.postMessage`, `reactions.add` with a bot token).
///
/// It is not implemented in this release. Every operation refuses with
/// `transport_unimplemented`, outcome not applied, so selecting it can never post anything.
#[derive(Clone, Copy, Debug, Default)]
pub struct WebApiTransport;

impl WebApiTransport {
    fn refusal() -> TransportError {
        TransportError::not_applied(
            "transport_unimplemented",
            "the Slack Web API transport is not implemented in this release; configure the cli transport",
            false,
        )
    }
}

impl SlackTransport for WebApiTransport {
    fn open_stream(
        &mut self,
        _spec: &SubscribeSpec,
    ) -> Result<Box<dyn EventStream>, TransportError> {
        Err(Self::refusal())
    }

    fn post_message(&mut self, _post: &PostMessage<'_>) -> Result<String, TransportError> {
        Err(Self::refusal())
    }

    fn add_reaction(
        &mut self,
        _app_id: &AppId,
        _channel_id: &str,
        _message_ts: &str,
        _name: &str,
    ) -> Result<bool, TransportError> {
        Err(Self::refusal())
    }

    fn list_messages(
        &mut self,
        _query: &ListQuery<'_>,
    ) -> Result<Vec<ListedMessage>, TransportError> {
        Err(Self::refusal())
    }

    fn resolve_sender(&mut self, _app_id: &AppId, _query: &str) -> Result<String, TransportError> {
        Err(Self::refusal())
    }

    fn list_members(
        &mut self,
        _app_id: &AppId,
        _channel_id: &str,
    ) -> Result<Vec<String>, TransportError> {
        Err(Self::refusal())
    }

    fn sender_is_human(
        &mut self,
        _app_id: &AppId,
        _sender_id: &str,
    ) -> Result<bool, TransportError> {
        Err(Self::refusal())
    }
}
