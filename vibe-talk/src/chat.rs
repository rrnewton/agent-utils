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
///
/// `Clone` so one failed read can be reported to every caller waiting on it: a channel's refresh
/// is shared, and so is its failure. `#210 voice-catchup-speed`.
#[derive(Clone, Debug, thiserror::Error)]
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
    /// A forward cursor the backend can no longer answer for. `#203 incremental-refresh`.
    ///
    /// Issued by an earlier process, older than the changes the backend still remembers, failing
    /// its verification, or more changed than one catch-up reads. Never transient: the same cursor
    /// fails the same way again. The caller reads the newest page instead, which issues a fresh one.
    #[error("cannot continue from that position: {0}")]
    CursorExpired(String),
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
            Self::CursorExpired(detail) => {
                format!("{provider} can no longer continue from that position: {detail}")
            }
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

    /// Whether the same request, sent again later, could succeed.
    ///
    /// `#195 send-resilience`. A lost connection, a rate limit, a timeout, or a provider's 5xx is a
    /// moment; a refusal — ours, or the provider's 4xx — is an answer, and sending it again asks
    /// the same question. An answer this server could not read is not transient either: it came
    /// from a provider that is up, and repeating the request repeats the answer.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self.cause() {
            Self::Transport(_) | Self::RateLimited(_) => true,
            Self::Status { status, .. } => *status >= 500 || *status == 408 || *status == 429,
            Self::Shape(_) | Self::Refused(_) | Self::CursorExpired(_) | Self::Provider { .. } => {
                false
            }
        }
    }

    /// Whether a post that failed this way may have been posted anyway.
    ///
    /// `#264 voice-send-approval`. Not the question [`ChatError::is_transient`] answers. A lost
    /// connection or a timeout may have cut off the answer after the provider took the post; a 5xx
    /// may come from a gateway in front of a provider that took it; and an answer this server
    /// could not read was a success whose body did not parse. A rate limit, a 4xx and this
    /// server's own refusal each say that the post was not taken.
    #[must_use]
    pub fn may_have_delivered(&self) -> bool {
        match self.cause() {
            Self::Transport(_) | Self::Shape(_) => true,
            Self::Status { status, .. } => *status >= 500,
            Self::RateLimited(_)
            | Self::Refused(_)
            | Self::CursorExpired(_)
            | Self::Provider { .. } => false,
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

/// How strongly a provider recognises an operator-supplied channel reference.
///
/// A deployment with several providers has to decide which one a pasted link belongs to before
/// any of them is asked to register it. Only the provider can recognise its own link shapes and id
/// syntax, so the decision is asked of each provider rather than guessed from the text centrally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceClaim {
    /// The reference is unmistakably this provider's: a link to its host, or its own id syntax.
    Certain,
    /// The provider cannot tell. A bridge that resolves references upstream answers this.
    Possible,
    /// The reference cannot be this provider's.
    Never,
}

/// What one automatic upstream clear did, or in a dry run would have done. `#232 upstream-mark-read`.
///
/// The provider's own answer, carried through unchanged: the server logs it and shows it in
/// Settings, and never infers a cursor the provider did not report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamClearOutcome {
    /// What the cursor covers at the provider, such as `space`.
    pub scope: String,
    /// The instant the cursor was moved to (or would have been, in a dry run), as the provider
    /// wrote it.
    pub target: String,
    /// The provider's cursor before this call, when it could be read. `None` means it had none.
    pub previous: Option<String>,
    /// Whether the cursor moved, or in a dry run whether it WOULD have: false when it was already
    /// at or past `target`. A dry run never writes, whatever this says.
    pub advanced: bool,
    /// Whether this was a dry run, so nothing was written at the provider.
    pub dry_run: bool,
}

/// How much text one post may carry at a provider, counted the way that provider counts it.
/// `#263 post-split-per-provider`.
///
/// A post longer than this is cut into parts that each fit ([`crate::discord::split`]); one that
/// fits is sent whole. Until this existed every post was cut at Discord's 2000 characters whatever
/// the provider, so a long message to a service that takes sixteen times that arrived as several.
///
/// Both measures are of the text **as the provider receives it**, after [`PostEscaping`]: a
/// provider that turns `<` into `&lt;` counts four characters where the writer typed one, and a
/// part cut to fit the writer's count could still be refused or truncated upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostLimit {
    /// Most characters (Unicode scalar values) in one post; `None` counts no characters.
    pub max_chars: Option<usize>,
    /// Most UTF-8 bytes in one post; `None` counts no bytes.
    pub max_bytes: Option<usize>,
    /// What the provider's client does to the text before sending it.
    pub escaping: PostEscaping,
}

/// What a provider's client does to a post's text on its way out, as far as its length goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PostEscaping {
    /// Sent exactly as written.
    #[default]
    Verbatim,
    /// `&`, `<` and `>` are sent as `&amp;`, `&lt;` and `&gt;`. Slack's client does this to every
    /// one of them but a user mention; counting a mention as escaped too only ever cuts a part
    /// shorter than it had to be, never longer.
    HtmlEntities,
}

impl PostLimit {
    /// At most `max` characters, sent verbatim.
    #[must_use]
    pub const fn chars(max: usize) -> Self {
        Self {
            max_chars: Some(max),
            max_bytes: None,
            escaping: PostEscaping::Verbatim,
        }
    }

    /// At most `max` UTF-8 bytes, sent verbatim.
    #[must_use]
    pub const fn bytes(max: usize) -> Self {
        Self {
            max_chars: None,
            max_bytes: Some(max),
            escaping: PostEscaping::Verbatim,
        }
    }

    /// The same limit, counted after `escaping`.
    #[must_use]
    pub const fn escaped(self, escaping: PostEscaping) -> Self {
        Self { escaping, ..self }
    }

    /// What one written character costs once escaped: `(characters, bytes)`.
    #[must_use]
    pub fn cost(&self, c: char) -> (usize, usize) {
        match (self.escaping, c) {
            (PostEscaping::HtmlEntities, '&') => (5, 5),
            (PostEscaping::HtmlEntities, '<' | '>') => (4, 4),
            _ => (1, c.len_utf8()),
        }
    }

    /// What `text` costs once escaped: `(characters, bytes)`.
    #[must_use]
    pub fn measure(&self, text: &str) -> (usize, usize) {
        text.chars().fold((0, 0), |(chars, bytes), c| {
            let (more_chars, more_bytes) = self.cost(c);
            (chars + more_chars, bytes + more_bytes)
        })
    }

    /// Whether a post of `chars` characters and `bytes` bytes, as sent, is within this limit.
    #[must_use]
    pub fn within(&self, chars: usize, bytes: usize) -> bool {
        self.max_chars.is_none_or(|max| chars <= max)
            && self.max_bytes.is_none_or(|max| bytes <= max)
    }

    /// Whether `text` may go as one post.
    #[must_use]
    pub fn admits(&self, text: &str) -> bool {
        let (chars, bytes) = self.measure(text);
        self.within(chars, bytes)
    }

    /// Why `text` may not go as one post, for a refusal; `None` when it may.
    #[must_use]
    pub fn refusal(&self, text: &str) -> Option<String> {
        let (chars, bytes) = self.measure(text);
        let escaped = if self.escaping == PostEscaping::Verbatim {
            ""
        } else {
            " once escaped"
        };
        if let Some(max) = self.max_chars.filter(|max| chars > *max) {
            return Some(format!(
                "message content is {chars} characters{escaped}; the chat API accepts at most {max}"
            ));
        }
        self.max_bytes.filter(|max| bytes > *max).map(|max| {
            format!(
                "message content is {bytes} UTF-8 bytes{escaped}; the chat API accepts at most {max}"
            )
        })
    }
}

/// A read of a channel's most recent messages, and how current the provider says it is.
/// `#208 persistent-cache`, milestone `bridge-recent-flat`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentRead {
    /// Up to the requested number of most recent messages, oldest first.
    pub messages: Vec<Message>,
    /// The instant, RFC 3339, up to which the provider knows `messages` is complete, when it
    /// served a copy it could not bring up to date just now. `None` means now. The same field, and
    /// the same meaning, as [`crate::threads::TimelinePage::as_of`].
    pub as_of: Option<String>,
    /// Whether the provider PROVED that `messages` reach back to the channel's first message.
    ///
    /// Never inferred from a short read: a provider can drop rows it cannot represent, so fewer
    /// messages than asked for is not a proof. False by default, and for every provider today.
    pub exhausted: bool,
}

/// Which messages of a channel a range read covers. remote-ops-audit W10 `#287 bulk-reads`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadScope {
    /// The channel's own messages, exactly as [`ChatClient::fetch_page`] reads them.
    Main,
    /// Every message of the channel, thread replies included, in one history.
    Flat,
    /// One thread of the channel: its root and its replies.
    Thread(String),
}

impl ReadScope {
    /// The scope in words, for a refusal.
    #[must_use]
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Main => "channel's own messages",
            Self::Flat => "whole history, replies included,",
            Self::Thread(_) => "thread",
        }
    }
}

/// A half-open window of provider time, in Unix milliseconds: `since_ms <= created < until_ms`.
/// remote-ops-audit W10 `#287 bulk-reads`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeRange {
    /// The first instant inside the window.
    pub since_ms: i64,
    /// The first instant past it. `None` means now: the instant the read began, so a message
    /// created while it runs may or may not be in it.
    pub until_ms: Option<i64>,
}

impl TimeRange {
    /// Whether an instant is inside the window.
    #[must_use]
    pub fn contains(&self, ms: i64) -> bool {
        ms >= self.since_ms && self.until_ms.is_none_or(|until| ms < until)
    }
}

/// Which end of a [`TimeRange`] a range read starts from, and so which way its continuation runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeOrder {
    /// From `since` toward `until`: the oldest messages of the window first, as a forward walk
    /// through a span (`/page?since=`) takes them.
    OldestFirst,
    /// From `until`, or now, toward `since`: the newest messages of the window first, as a count
    /// or a backfill takes them.
    NewestFirst,
}

/// One step of a range read. remote-ops-audit W10 `#287 bulk-reads`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RangeRead {
    /// At most the number asked for, every one inside the window, oldest first: the window's
    /// oldest ones for [`RangeOrder::OldestFirst`], its newest for [`RangeOrder::NewestFirst`].
    pub messages: Vec<Message>,
    /// Where the next step starts, in the same order. Opaque, and valid only for the same
    /// channel, scope, window and order. `None` when the provider has nowhere further to go,
    /// which on its own proves nothing: see [`RangeRead::exhausted`].
    pub next: Option<String>,
    /// Whether the provider PROVED there is nothing more in the window beyond `messages`, in the
    /// direction of travel: Slack's `has_more: false`, or a bridge's exhaustive answer.
    ///
    /// Never inferred from a short step: a provider can drop rows it cannot represent, and a page
    /// can be cut short, so fewer messages than asked for is not a proof. False by default.
    pub exhausted: bool,
    /// The instant, RFC 3339, up to which the provider knows its answer is complete, when it
    /// served a copy it could not bring up to date just now. `None` means now. As
    /// [`RecentRead::as_of`].
    pub as_of: Option<String>,
}

/// Whether a provider answers a bulk read itself, or the trait's default answers it with
/// single-channel reads that cost what they always did. remote-ops-audit W10 `#287 bulk-reads`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Support {
    /// The trait's default body: correct, and no cheaper than the reads it is made of. A caller
    /// that would use the read only because it is cheap must not use it.
    #[default]
    Emulated,
    /// The provider's own read, cheaper than the default.
    Native,
}

/// Which bulk reads a provider answers natively for a channel, and how many rows one of its
/// upstream calls returns. remote-ops-audit W10 `#287 bulk-reads`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadCaps {
    /// [`ChatClient::fetch_range`] of the channel's own messages ([`ReadScope::Main`]).
    pub range: Support,
    /// [`ChatClient::fetch_messages`]: several messages by id in fewer upstream calls than one
    /// each. remote-ops-audit W11.
    pub by_ids: Support,
    /// [`ChatClient::channel_heads`]: the newest message of several channels in fewer upstream
    /// calls than a read of each. A caller that would ask for heads only as a cheap gate (the live
    /// poller, a catch-up deciding what to read again) asks only when this is native.
    /// remote-ops-audit W12.
    pub heads: Support,
    /// Whether this provider's heads can vouch for edits and deletes as well as additions: may
    /// carry [`ChannelHead::unchanged_since_ms`]. Only such heads can let a catch-up keep a held
    /// copy instead of reading it again, so a catch-up asks for heads only when this is true (and
    /// `heads` is native). False for Discord, whose head moves only when a message is added.
    /// remote-ops-audit W12.
    pub heads_cover_edits: bool,
    /// The most rows one upstream call returns, for a caller counting a budget in calls: Discord's
    /// 100 unless the provider says otherwise.
    pub page_rows: u16,
}

impl Default for ReadCaps {
    fn default() -> Self {
        Self {
            range: Support::Emulated,
            by_ids: Support::Emulated,
            heads: Support::Emulated,
            heads_cover_edits: false,
            page_rows: crate::discord::http::DISCORD_MAX_LIMIT,
        }
    }
}

/// What a lookup by id found for one of the ids asked for. remote-ops-audit W11 `#287 bulk-reads`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The message, as the provider has it now. Boxed: a lookup's answers are mostly this.
    Found(Box<Message>),
    /// The provider PROVED there is no such message where it was looked for (the channel's own
    /// messages, or the thread asked for): it answered that it has none by that id, or it read,
    /// without a gap, the whole stretch of the channel the id would fall in, and the id was not in
    /// it.
    Absent,
    /// Not where the lookup looked, which proves nothing: a window that did not reach back to it,
    /// a read cut short, a copy older than now, or a message the provider has but cannot
    /// represent. A caller that needs the message looks it up the way it would one at a time.
    Unseen,
}

impl Lookup {
    /// The message, when one was found.
    #[must_use]
    pub fn found(self) -> Option<Message> {
        match self {
            Self::Found(message) => Some(*message),
            Self::Absent | Self::Unseen => None,
        }
    }
}

/// How many [`ChatClient::fetch_thread_message`] calls [`emulated_messages`] keeps in flight: what
/// speech preparation has always used for one thread (`crate::read_ahead`).
pub const LOOKUPS_IN_FLIGHT: usize = 4;

/// What a provider knows of a channel's newest message without reading the channel: enough to tell
/// whether it moved since it was last read. remote-ops-audit W12 `#287 bulk-reads`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelHead {
    /// The newest of the channel's own messages, as [`ChatClient::fetch_page`] lists them; `None`
    /// when it has none. It may name a message deleted since (Discord's `last_message_id` does),
    /// so all it says is that nothing NEWER was added: a message created after it would have
    /// taken its place.
    pub newest_id: Option<MessageId>,
    /// When that message was created, in Unix milliseconds, where the provider says.
    pub newest_at_ms: Option<i64>,
    /// The latest activity in the channel, thread replies included, in Unix milliseconds, where
    /// the provider knows it.
    pub activity_ms: Option<i64>,
    /// An instant, in Unix milliseconds, after which the provider KNOWS nothing in the channel
    /// changed: no message created, edited or deleted, a thread reply included. `None` when it
    /// cannot say, which is always so for a provider whose head moves only when a message is added
    /// (Discord).
    ///
    /// Only a head that carries this can prove a held copy of the channel still current;
    /// `newest_id` alone proves that nothing was added, never that nothing was edited or deleted.
    pub unchanged_since_ms: Option<i64>,
    /// As [`RecentRead::as_of`]: the instant, RFC 3339, up to which the provider knows this head is
    /// current, when it answered from a copy it could not bring up to date. `None` means now. A
    /// head with an `as_of` proves nothing about the channel after it, so no caller acts on one.
    pub as_of: Option<String>,
}

impl ChannelHead {
    /// Whether this head is the provider's word for now, rather than an older copy's.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.as_of.is_none()
    }
}

/// One answer per channel of a [`ChatClient::channel_heads`] call, in the order asked.
pub type HeadAnswers = Vec<(ChannelId, Result<ChannelHead, ChatError>)>;

/// How many [`ChatClient::fetch_recent`] reads [`emulated_heads`] keeps in flight: the bounded
/// fan-out every bulk default keeps, per provider.
pub const HEADS_IN_FLIGHT: usize = 4;

/// Read and post access to configured chat channels.
#[async_trait]
pub trait ChatClient: Send + Sync {
    /// Human-readable name of the source chat service, including when accessed through a bridge.
    fn provider_name(&self) -> &str {
        "Chat"
    }

    /// Whether `source`, an operator-supplied channel link or reference, belongs to this provider.
    ///
    /// Consulted only when several providers are configured. `Possible` by default, so a bridge
    /// that resolves arbitrary references keeps working as the fallback for anything another
    /// provider does not claim with certainty.
    fn claims_source(&self, _source: &str) -> SourceClaim {
        SourceClaim::Possible
    }

    /// The account this provider posts as, when it is known without asking the provider again.
    ///
    /// The page uses it to draw this server's own posts as the owner's. `None` means not known
    /// yet, which degrades to learning it from the first post rather than guessing.
    fn self_author_id(&self) -> Option<String> {
        None
    }

    /// The owner's own account at this provider, when the deployment has said what it is.
    fn owner_author_id(&self) -> Option<String> {
        None
    }

    /// Whether this backend supports channel and thread timeline views.
    fn supports_threading(&self) -> bool {
        false
    }

    /// Whether a reply to a message on a channel's own timeline is posted in that message's
    /// thread, starting one when it has none, because the provider has no reply pointer of its
    /// own: Slack. Such a reply and a post into the message's thread are then the same act, so
    /// the page does not offer starting a thread from Reply as a separate choice.
    /// `#252 thread-button-state`. False by default.
    fn replies_in_thread(&self) -> bool {
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

    /// What changed in a view since a generic position, for a backend whose
    /// [`ChatClient::fetch_timeline`] does not answer `after` itself. `#203 incremental-refresh`.
    ///
    /// `request` carries no cursor. The default reads newest pages and keeps what is newer than
    /// the position: correct for every backend, with `complete: false`, and no cheaper than a
    /// newest read. The router forwards it to the channel's provider, so a provider may override
    /// it with something cheaper. Answers the delta page and the position that continues it.
    ///
    /// # Errors
    ///
    /// [`ChatError::CursorExpired`] when more changed than one catch-up reads; otherwise the
    /// errors of [`ChatClient::fetch_timeline`].
    async fn fetch_timeline_since(
        &self,
        channel: &ChannelId,
        request: &crate::threads::TimelineRequest,
        since: &crate::timeline_forward::GenericPosition,
    ) -> Result<crate::timeline_forward::GenericDelta, ChatError> {
        crate::timeline_forward::generic_since(self, channel, request, since).await
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

    /// Whether this provider can list the channels its account can see. `#19 channel-browser`.
    ///
    /// False by default: only a registration-capable bridge that answers the directory contract
    /// offers it, because a listed channel is only useful if it can then be registered.
    fn supports_channel_discovery(&self) -> bool {
        false
    }

    /// Read one page of the channels this provider's account can see.
    ///
    /// # Errors
    ///
    /// Returns [`ChatError`] when discovery is unsupported, denied, or fails.
    async fn discover_channels(
        &self,
        _request: &crate::directory::DirectoryRequest,
    ) -> Result<crate::directory::DirectoryPage, ChatError> {
        Err(ChatError::Refused(
            "the configured chat provider cannot list its channels".to_owned(),
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

    /// [`ChatClient::fetch_recent`], with what the provider says about how current it is.
    /// `#208 persistent-cache`.
    ///
    /// The default wraps `fetch_recent` and claims nothing: no `as_of`, and not `exhausted`. A
    /// provider that serves the newest messages from a copy it keeps overrides it to say how old
    /// that copy is. The router forwards it to the channel's own provider.
    ///
    /// # Errors
    ///
    /// As [`ChatClient::fetch_recent`].
    async fn fetch_recent_dated(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<RecentRead, ChatError> {
        Ok(RecentRead {
            messages: self.fetch_recent(channel, limit).await?,
            as_of: None,
            exhausted: false,
        })
    }

    /// Which bulk reads this provider answers natively for `channel`. remote-ops-audit W10
    /// `#287 bulk-reads`.
    ///
    /// Everything [`Support::Emulated`] by default, at Discord's 100 rows a call. The router
    /// answers with the channel's own provider's capabilities.
    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        ReadCaps::default()
    }

    /// One step of a read of the messages of `scope` created inside `range`. remote-ops-audit W10
    /// `#287 bulk-reads`.
    ///
    /// At most `limit` messages, oldest first, from the end of the window `order` names; `cursor`
    /// is a step's [`RangeRead::next`], for the same channel, scope, window and order. A native
    /// read ([`ReadCaps::range`]) costs about one upstream call per [`ReadCaps::page_rows`]
    /// rows it returns, never anything for the messages outside the window.
    ///
    /// The default is [`emulated_range`]: one [`ChatClient::fetch_page`] from the window's edge,
    /// trimmed to the window, exactly the read `/page?since=` has always made. It reads only the
    /// channel's own messages, and refuses the other scopes.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a scope or a cursor this provider cannot answer; otherwise the
    /// errors of [`ChatClient::fetch_page`].
    async fn fetch_range(
        &self,
        channel: &ChannelId,
        scope: &ReadScope,
        range: TimeRange,
        order: RangeOrder,
        limit: u16,
        cursor: Option<&str>,
    ) -> Result<RangeRead, ChatError> {
        emulated_range(self, channel, scope, range, order, limit, cursor).await
    }

    /// The messages of `channel`, or of one `thread` of it, with the ids asked for: one [`Lookup`]
    /// per id, in the order asked, a repeated id answered each time. remote-ops-audit W11
    /// `#287 bulk-reads`.
    ///
    /// A native lookup ([`ReadCaps::by_ids`]) costs fewer upstream calls than one per id, about
    /// one per run of ids that sit close together, and answers [`Lookup::Absent`] only where the
    /// provider proves it.
    ///
    /// The default is [`emulated_messages`]: in a thread, one [`ChatClient::fetch_thread_message`]
    /// per id, [`LOOKUPS_IN_FLIGHT`] at a time, the first failure failing the lookup; among the
    /// channel's own messages, one window of the newest
    /// ([`crate::discord::http::DISCORD_MAX_LIMIT`], through [`ChatClient::fetch_recent_dated`])
    /// searched for each id.
    ///
    /// # Errors
    ///
    /// The errors of the reads it is made of: a lookup that cannot answer for every id fails
    /// rather than answer for some.
    async fn fetch_messages(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        ids: &[MessageId],
    ) -> Result<Vec<Lookup>, ChatError> {
        emulated_messages(self, channel, thread, ids).await
    }

    /// The newest message of each of `channels`, without reading their messages: one answer per
    /// channel, in the order asked, a repeated channel answered each time. remote-ops-audit W12
    /// `#287 bulk-reads`.
    ///
    /// A native head read ([`ReadCaps::heads`]) costs fewer upstream calls than a read of each
    /// channel (Discord's channel list is one call per server), so a caller may use it as a cheap
    /// gate: leave alone a channel whose head has not moved. An emulated one costs exactly what
    /// reading each channel's newest message costs, so a caller that would ask only because it is
    /// cheap does not ask.
    ///
    /// The default is [`emulated_heads`]: one [`ChatClient::fetch_recent`] of one message per
    /// channel, [`HEADS_IN_FLIGHT`] at a time. The router asks each provider once, for its own
    /// channels only.
    ///
    /// A channel that cannot be answered is an `Err` in its own place, never a failure of the
    /// whole call.
    async fn channel_heads(&self, channels: &[ChannelId]) -> HeadAnswers {
        emulated_heads(self, channels).await
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

    /// Whether a post carrying an idempotency key is posted at most once, however often it is sent.
    ///
    /// `#195 send-resilience`. A post whose answer is lost — a timeout, a dropped connection, a
    /// gateway's 502 — may or may not have landed, and sending it again is safe only where
    /// something upstream recognises the second request as the first. True only where the key
    /// provably reaches such a thing; the page retries an ambiguous failure on its own only then.
    ///
    /// False by default, and for every provider whose post API has no such key: the keyed methods
    /// below then ignore the key, which is honest exactly because this says so.
    fn supports_idempotent_posts(&self) -> bool {
        false
    }

    /// [`ChatClient::post_message`], carrying the idempotency key for this one part.
    ///
    /// The same key, for the same text, must name the same post on every attempt; see
    /// [`ChatClient::supports_idempotent_posts`]. The default ignores it.
    ///
    /// # Errors
    ///
    /// As [`ChatClient::post_message`].
    async fn post_message_keyed(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
        _key: Option<&str>,
    ) -> Result<Message, ChatError> {
        self.post_message(channel, content, reply_to).await
    }

    /// [`ChatClient::post_in_thread`], carrying the idempotency key for this one part.
    ///
    /// # Errors
    ///
    /// As [`ChatClient::post_in_thread`].
    async fn post_in_thread_keyed(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        content: &str,
        reply_to: Option<&MessageId>,
        _key: Option<&str>,
    ) -> Result<Message, ChatError> {
        self.post_in_thread(channel, thread_id, content, reply_to)
            .await
    }

    /// The most text one post to `channel` may carry. `#263 post-split-per-provider`.
    ///
    /// [`crate::ops::reply_scoped`] cuts a longer post into parts that each fit, and sends one
    /// that fits whole. The default is Discord's 2000 characters
    /// ([`crate::discord::http::DISCORD_MAX_CONTENT_LEN`]): the limit every post was cut at before
    /// a provider could say otherwise, so one that does not say keeps that behaviour.
    fn post_limit(&self, _channel: &ChannelId) -> PostLimit {
        PostLimit::chars(crate::discord::http::DISCORD_MAX_CONTENT_LEN)
    }

    /// The thread the rest of a post that had to be cut continues in, once its first part,
    /// `first`, was posted to the conversation itself. `#263 post-split-per-provider`.
    ///
    /// `None`, the default, posts every part into the conversation one after another, and only the
    /// first answers the reply target. That is Discord's behaviour: this route cannot start a
    /// thread from a message it has just posted. A provider where any message can carry a thread
    /// answers the thread `first` is in or now roots, and the rest is posted there, so a long post
    /// reads as one message in the conversation with its continuation under it.
    fn continuation_thread(&self, _channel: &ChannelId, _first: &Message) -> Option<String> {
        None
    }

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

    /// Whether this provider can clear its own unread state automatically, as vibe-talk ingests
    /// messages. `#232 upstream-mark-read`.
    ///
    /// Separate from [`ChatClient::supports_upstream_read_mark`], the owner's own manual mark:
    /// the automatic path runs with nobody watching, so it needs a provider endpoint that yields
    /// to the owner's actions, supports a dry run, and refuses a channel where a provider-wide
    /// cursor would clear messages this server never received. False by default.
    fn supports_upstream_clear(&self) -> bool {
        false
    }

    /// Move the provider's read cursor through `through`, or in a dry run report where it would
    /// move. `#232 upstream-mark-read`.
    ///
    /// Never moves the cursor backwards: the provider compares with its current cursor first.
    /// vibe-talk's own read marks and Done state are not touched.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] when unsupported; otherwise the provider's failure. A provider
    /// answers HTTP 409 for a channel the cursor cannot be moved for, and HTTP 404 when it no
    /// longer holds the message.
    async fn clear_upstream_through(
        &self,
        _channel: &ChannelId,
        _through: &MessageId,
        _dry_run: bool,
    ) -> Result<UpstreamClearOutcome, ChatError> {
        Err(ChatError::Refused(
            "the configured chat provider cannot clear its unread state automatically".to_owned(),
        ))
    }
}

/// [`ChatClient::fetch_range`]'s default: a range read made of one [`ChatClient::fetch_page`].
/// remote-ops-audit W10 `#287 bulk-reads`.
///
/// Oldest first, the page starts just below `since` (`after` the id of the millisecond before
/// it, Discord's `after` being exclusive); newest first, just below `until` (`before` the id of
/// that instant), or at the newest message when the window runs to now. A `cursor` replaces that
/// edge with the message the last step reached. The page is then trimmed to the window by each
/// message's creation instant ([`Message::created_ms`]); one whose instant cannot be read is
/// kept, as `/page` has always kept it. Exactly the read and the trim `/page?since=` made before
/// this method existed, so a provider without a range read of its own answers it as before.
///
/// `next` is the id of the last message the page reached, unless the page already reached past
/// the window's far end. Never `exhausted`: a page that came back short, or one that reached past
/// the window, is the provider's word for it, not a proof this server can stamp.
///
/// # Errors
///
/// [`ChatError::Refused`] for any scope but [`ReadScope::Main`]: a channel's threads have no time
/// window this default can ask for. Otherwise the errors of [`ChatClient::fetch_page`].
pub async fn emulated_range<C: ChatClient + ?Sized>(
    client: &C,
    channel: &ChannelId,
    scope: &ReadScope,
    range: TimeRange,
    order: RangeOrder,
    limit: u16,
    cursor: Option<&str>,
) -> Result<RangeRead, ChatError> {
    if *scope != ReadScope::Main {
        return Err(ChatError::Refused(format!(
            "this backend cannot read a channel's {} by time",
            scope.describe()
        )));
    }
    let cursor = cursor.map(|raw| MessageId(raw.to_owned()));
    let raw = match order {
        RangeOrder::OldestFirst => {
            let after = cursor.unwrap_or_else(|| MessageId::at_time_ms(range.since_ms - 1));
            client
                .fetch_page(channel, limit, None, Some(&after))
                .await?
        }
        RangeOrder::NewestFirst => {
            let before = cursor.or_else(|| range.until_ms.map(MessageId::at_time_ms));
            client
                .fetch_page(channel, limit, before.as_ref(), None)
                .await?
        }
    };
    let next = match order {
        RangeOrder::OldestFirst => {
            let past = raw.iter().any(|m| {
                m.created_ms()
                    .is_some_and(|ms| range.until_ms.is_some_and(|until| ms >= until))
            });
            raw.last().filter(|_| !past)
        }
        RangeOrder::NewestFirst => {
            let past = raw
                .iter()
                .any(|m| m.created_ms().is_some_and(|ms| ms < range.since_ms));
            raw.first().filter(|_| !past)
        }
    }
    .map(|m| m.id.0.clone());
    let messages = raw
        .into_iter()
        .filter(|m| m.created_ms().is_none_or(|ms| range.contains(ms)))
        .collect();
    Ok(RangeRead {
        messages,
        next,
        exhausted: false,
        as_of: None,
    })
}

/// [`ChatClient::fetch_messages`]'s default: a lookup by id made of the reads that answered one
/// id before it existed. remote-ops-audit W11 `#287 bulk-reads`.
///
/// In a thread, one [`ChatClient::fetch_thread_message`] per id, [`LOOKUPS_IN_FLIGHT`] at a time,
/// in the order asked; the first failure fails the lookup, as it failed speech preparation, and a
/// provider's refusal of a message it does not have stays that refusal. Every answer is
/// [`Lookup::Found`].
///
/// Among the channel's own messages, one [`ChatClient::fetch_recent_dated`] of the newest
/// [`crate::discord::http::DISCORD_MAX_LIMIT`], searched for each id. An id not in that window is
/// [`Lookup::Unseen`]: a window proves nothing about what is older than it. Only a window the
/// provider proved reaches back to the channel's first message, and read now rather than from an
/// older copy, proves an id absent.
///
/// Nothing is read for an empty list of ids.
///
/// # Errors
///
/// The errors of [`ChatClient::fetch_thread_message`] and [`ChatClient::fetch_recent_dated`].
pub async fn emulated_messages<C: ChatClient + ?Sized>(
    client: &C,
    channel: &ChannelId,
    thread: Option<&str>,
    ids: &[MessageId],
) -> Result<Vec<Lookup>, ChatError> {
    use futures_util::{StreamExt as _, TryStreamExt as _};

    if ids.is_empty() {
        return Ok(Vec::new());
    }
    match thread {
        Some(thread) => {
            // Made before they are streamed: a closure handing out the provider's boxed futures
            // inside the stream is not general enough over their lifetimes for `async_trait`.
            let lookups: Vec<_> = ids
                .iter()
                .map(|id| client.fetch_thread_message(channel, thread, id))
                .collect();
            futures_util::stream::iter(lookups)
                .buffered(LOOKUPS_IN_FLIGHT)
                .map_ok(|message| Lookup::Found(Box::new(message)))
                .try_collect()
                .await
        }
        None => {
            let window = client
                .fetch_recent_dated(channel, crate::discord::http::DISCORD_MAX_LIMIT)
                .await?;
            let proves_absence = window.exhausted && window.as_of.is_none();
            Ok(ids
                .iter()
                .map(|id| match window.messages.iter().find(|m| m.id == *id) {
                    Some(message) => Lookup::Found(Box::new(message.clone())),
                    None if proves_absence => Lookup::Absent,
                    None => Lookup::Unseen,
                })
                .collect())
        }
    }
}

/// [`ChatClient::channel_heads`]'s default: a head read made of the read of each channel's newest
/// message. remote-ops-audit W12 `#287 bulk-reads`.
///
/// One [`ChatClient::fetch_recent`] of one message per channel, [`HEADS_IN_FLIGHT`] at a time, in
/// the order asked; each read's failure is that channel's answer. The head is the newest message
/// the read returned, with its creation instant; it says nothing about activity or changes, and
/// claims no `as_of`. Nothing is read for an empty list.
pub async fn emulated_heads<C: ChatClient + ?Sized>(
    client: &C,
    channels: &[ChannelId],
) -> HeadAnswers {
    use futures_util::StreamExt as _;

    // Made before they are streamed, as in `emulated_messages`.
    let reads: Vec<_> = channels
        .iter()
        .map(|channel| client.fetch_recent(channel, 1))
        .collect();
    let answers: Vec<_> = futures_util::stream::iter(reads)
        .buffered(HEADS_IN_FLIGHT)
        .collect()
        .await;
    channels
        .iter()
        .cloned()
        .zip(answers.into_iter().map(|read| {
            read.map(|messages| {
                let newest = messages.last();
                ChannelHead {
                    newest_id: newest.map(|m| m.id.clone()),
                    newest_at_ms: newest.and_then(Message::created_ms),
                    ..ChannelHead::default()
                }
            })
        }))
        .collect()
}

/// Put head answers given for parts of a list of channels back in the list's order: each part is
/// the positions its channels held in `channels` and what was answered for them, in that order.
/// remote-ops-audit W12 `#287 bulk-reads`.
///
/// A position no part answered, or answered for a different channel, is a [`ChatError::Shape`] in
/// its own place: a provider that skips or reorders channels must not have one channel's head
/// taken for another's.
#[must_use]
pub fn heads_in_order(
    channels: &[ChannelId],
    parts: Vec<(Vec<usize>, HeadAnswers)>,
) -> HeadAnswers {
    let mut answers: Vec<Option<Result<ChannelHead, ChatError>>> = vec![None; channels.len()];
    for (positions, answered) in parts {
        let mut answered = answered.into_iter();
        for at in positions {
            let Some(slot) = answers.get_mut(at) else {
                continue;
            };
            *slot = Some(match answered.next() {
                Some((channel, head)) if channel == channels[at] => head,
                Some(_) => Err(ChatError::Shape(
                    "a head read answered for a different channel than the one in its place"
                        .to_owned(),
                )),
                None => Err(ChatError::Shape(
                    "a head read answered for fewer channels than it was asked about".to_owned(),
                )),
            });
        }
    }
    channels
        .iter()
        .cloned()
        .zip(answers.into_iter().map(|answer| {
            answer.unwrap_or_else(|| {
                Err(ChatError::Shape(
                    "no head was asked for in this channel's place".to_owned(),
                ))
            })
        }))
        .collect()
}

#[cfg(test)]
mod heads_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::discord::fake::FakeDiscord;

    const BASE_MS: i64 = 1_787_000_000_000;

    /// A provider that writes only what the trait requires, counts its recent reads and the most
    /// in flight at once, and logs each one.
    struct Plain {
        inner: FakeDiscord,
        in_flight: AtomicUsize,
        most_in_flight: AtomicUsize,
        reads: Mutex<Vec<(String, u16)>>,
    }

    impl Plain {
        fn new() -> Self {
            Self {
                inner: FakeDiscord::new(),
                in_flight: AtomicUsize::new(0),
                most_in_flight: AtomicUsize::new(0),
                reads: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ChatClient for Plain {
        async fn identity(&self) -> Result<ChatIdentity, ChatError> {
            self.inner.identity().await
        }

        async fn fetch_page(
            &self,
            channel: &ChannelId,
            limit: u16,
            before: Option<&MessageId>,
            after: Option<&MessageId>,
        ) -> Result<Vec<Message>, ChatError> {
            self.reads
                .lock()
                .expect("log")
                .push((channel.0.clone(), limit));
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_in_flight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.inner.fetch_page(channel, limit, before, after).await
        }

        async fn post_message(
            &self,
            channel: &ChannelId,
            content: &str,
            reply_to: Option<&MessageId>,
        ) -> Result<Message, ChatError> {
            self.inner.post_message(channel, content, reply_to).await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_default_reads_one_message_of_each_channel_four_at_a_time_in_order() {
        let provider = Plain::new();
        let mut channels = Vec::new();
        let mut newest = Vec::new();
        for n in 0..6_i64 {
            let channel = ChannelId(format!("{}", 100 + n));
            provider
                .inner
                .seed_at(&channel, "ada", "older", BASE_MS + n * 10_000);
            newest.push(provider.inner.seed_at(
                &channel,
                "ada",
                "newest",
                BASE_MS + n * 10_000 + 1_000,
            ));
            channels.push(channel);
        }
        let unknown = ChannelId("404".to_owned());
        let mut asked = channels.clone();
        asked.insert(2, unknown.clone());
        asked.push(channels[0].clone());

        let answers = provider.channel_heads(&asked).await;

        assert_eq!(
            answers.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
            asked,
            "one answer per channel, in the order asked, a repeat answered again"
        );
        let reads = provider.reads.lock().expect("log").clone();
        assert_eq!(reads.len(), asked.len(), "one read per channel asked");
        assert!(reads.iter().all(|(_, limit)| *limit == 1), "{reads:?}");
        assert_eq!(
            provider.most_in_flight.load(Ordering::SeqCst),
            HEADS_IN_FLIGHT
        );
        let heads: Vec<&ChannelHead> = answers
            .iter()
            .filter_map(|(_, head)| head.as_ref().ok())
            .collect();
        assert_eq!(
            heads.len(),
            asked.len() - 1,
            "only the unknown channel failed"
        );
        assert!(
            answers[2].1.is_err(),
            "the unknown channel fails in its own place"
        );
        for (head, id) in heads.iter().zip(newest.iter().chain([&newest[0]])) {
            assert_eq!(head.newest_id.as_ref(), Some(id));
            assert_eq!(head.newest_at_ms, id.created_at_ms());
            assert_eq!(head.activity_ms, None, "a read says nothing of activity");
            assert_eq!(head.unchanged_since_ms, None, "nor of edits and deletes");
            assert!(head.is_current());
        }

        provider.reads.lock().expect("log").clear();
        assert!(provider.channel_heads(&[]).await.is_empty());
        assert!(
            provider.reads.lock().expect("log").is_empty(),
            "nothing asked"
        );
    }

    #[tokio::test]
    async fn the_default_head_of_an_empty_channel_names_no_message() {
        let provider = Plain::new();
        let channel = ChannelId("7".to_owned());
        provider.inner.register_channel(&channel);
        let answers = provider.channel_heads(std::slice::from_ref(&channel)).await;
        assert_eq!(
            answers[0].1.as_ref().expect("read"),
            &ChannelHead::default()
        );
    }

    #[test]
    fn heads_go_back_in_the_order_asked_and_a_skipped_or_reordered_one_is_an_error() {
        let ids: Vec<ChannelId> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| ChannelId((*id).to_owned()))
            .collect();
        let head = |n: u64| ChannelHead {
            newest_id: Some(MessageId(n.to_string())),
            ..ChannelHead::default()
        };
        let answers = heads_in_order(
            &ids,
            vec![
                (
                    vec![0, 2],
                    vec![(ids[0].clone(), Ok(head(1))), (ids[2].clone(), Ok(head(3)))],
                ),
                (vec![1], vec![(ids[3].clone(), Ok(head(4)))]),
            ],
        );
        assert_eq!(answers[0].1.as_ref().ok(), Some(&head(1)));
        assert!(
            matches!(answers[1].1, Err(ChatError::Shape(_))),
            "another channel's head"
        );
        assert_eq!(answers[2].1.as_ref().ok(), Some(&head(3)));
        assert!(
            matches!(answers[3].1, Err(ChatError::Shape(_))),
            "never asked"
        );
        assert_eq!(
            answers.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn a_provider_that_says_nothing_reads_heads_as_it_always_read_channels() {
        assert_eq!(ReadCaps::default().heads, Support::Emulated);
        assert_eq!(
            FakeDiscord::new()
                .read_caps(&ChannelId("42".to_owned()))
                .heads,
            Support::Emulated
        );
    }
}

#[cfg(test)]
mod lookup_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::discord::fake::FakeDiscord;
    use crate::model::UserId;

    /// A provider with a thread of ten messages, and a channel window it may claim is the whole
    /// channel; it counts what it is asked, and the most thread lookups in flight at once.
    struct Threaded {
        main: FakeDiscord,
        exhausted: bool,
        as_of: Option<String>,
        windows: AtomicUsize,
        in_flight: AtomicUsize,
        most_in_flight: AtomicUsize,
        looked_up: Mutex<Vec<String>>,
    }

    impl Threaded {
        fn new(exhausted: bool, as_of: Option<&str>) -> Self {
            Self {
                main: FakeDiscord::new(),
                exhausted,
                as_of: as_of.map(str::to_owned),
                windows: AtomicUsize::new(0),
                in_flight: AtomicUsize::new(0),
                most_in_flight: AtomicUsize::new(0),
                looked_up: Mutex::new(Vec::new()),
            }
        }
    }

    fn reply(id: &str) -> Message {
        Message {
            id: MessageId(id.to_owned()),
            channel_id: ChannelId("42".to_owned()),
            author: "grace".to_owned(),
            author_id: UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: "2026-10-13T01:00:00Z".to_owned(),
            spoken_time: String::new(),
            reply_to: None,
            content: format!("reply {id}"),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            thread: None,
            reactions: None,
            heading: None,
        }
    }

    #[async_trait]
    impl ChatClient for Threaded {
        async fn fetch_thread_message(
            &self,
            _channel: &ChannelId,
            thread_id: &str,
            message_id: &MessageId,
        ) -> Result<Message, ChatError> {
            self.looked_up
                .lock()
                .expect("log")
                .push(format!("{thread_id}/{message_id}"));
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_in_flight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            if message_id.as_str().starts_with('r') {
                Ok(reply(message_id.as_str()))
            } else {
                Err(ChatError::Status {
                    status: 404,
                    body: "not in that thread".to_owned(),
                })
            }
        }

        async fn fetch_recent_dated(
            &self,
            channel: &ChannelId,
            limit: u16,
        ) -> Result<RecentRead, ChatError> {
            self.windows.fetch_add(1, Ordering::SeqCst);
            Ok(RecentRead {
                messages: self.main.fetch_recent(channel, limit).await?,
                as_of: self.as_of.clone(),
                exhausted: self.exhausted,
            })
        }

        async fn identity(&self) -> Result<ChatIdentity, ChatError> {
            self.main.identity().await
        }

        async fn fetch_page(
            &self,
            channel: &ChannelId,
            limit: u16,
            before: Option<&MessageId>,
            after: Option<&MessageId>,
        ) -> Result<Vec<Message>, ChatError> {
            self.main.fetch_page(channel, limit, before, after).await
        }

        async fn post_message(
            &self,
            channel: &ChannelId,
            content: &str,
            reply_to: Option<&MessageId>,
        ) -> Result<Message, ChatError> {
            self.main.post_message(channel, content, reply_to).await
        }
    }

    fn ids(list: &[&str]) -> Vec<MessageId> {
        list.iter().map(|id| MessageId((*id).to_owned())).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn in_a_thread_the_default_asks_once_per_id_four_at_a_time_in_order() {
        let provider = Threaded::new(false, None);
        let channel = ChannelId("42".to_owned());
        let asked = ids(&["r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r2"]);
        let answers = provider
            .fetch_messages(&channel, Some("t"), &asked)
            .await
            .expect("found");
        let bodies: Vec<String> = answers
            .into_iter()
            .map(|answer| answer.found().expect("found").content)
            .collect();
        assert_eq!(
            bodies,
            asked
                .iter()
                .map(|id| format!("reply {id}"))
                .collect::<Vec<_>>(),
            "one answer per id, in order, a repeat answered again"
        );
        assert_eq!(provider.looked_up.lock().expect("log").len(), 9);
        assert_eq!(
            provider.most_in_flight.load(Ordering::SeqCst),
            LOOKUPS_IN_FLIGHT
        );
        assert_eq!(provider.windows.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn in_a_thread_the_default_fails_with_the_providers_own_refusal() {
        let provider = Threaded::new(false, None);
        let error = provider
            .fetch_messages(&ChannelId("42".to_owned()), Some("t"), &ids(&["r1", "x9"]))
            .await
            .expect_err("a message the provider does not have fails the lookup, as it always did");
        assert!(
            matches!(error, ChatError::Status { status: 404, .. }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn among_the_channels_own_messages_the_default_reads_one_window_and_proves_nothing() {
        let provider = Threaded::new(false, None);
        let channel = ChannelId("42".to_owned());
        let kept = provider.main.seed(&channel, "ada", "in the window");
        let answers = provider
            .fetch_messages(
                &channel,
                None,
                &[kept.clone(), MessageId("404".to_owned()), kept.clone()],
            )
            .await
            .expect("read");
        assert_eq!(provider.windows.load(Ordering::SeqCst), 1, "one window");
        assert!(matches!(&answers[0], Lookup::Found(m) if m.id == kept));
        assert_eq!(answers[1], Lookup::Unseen, "not in a window is not absent");
        assert!(matches!(&answers[2], Lookup::Found(m) if m.id == kept));
        assert!(provider.looked_up.lock().expect("log").is_empty());

        assert!(provider
            .fetch_messages(&channel, None, &[])
            .await
            .expect("nothing")
            .is_empty());
        assert_eq!(provider.windows.load(Ordering::SeqCst), 1, "nothing asked");
    }

    #[tokio::test]
    async fn only_a_current_window_of_the_whole_channel_proves_an_id_absent() {
        let channel = ChannelId("42".to_owned());
        for (exhausted, as_of, expected) in [
            (true, None, Lookup::Absent),
            (true, Some("2026-10-13T01:00:00Z"), Lookup::Unseen),
            (false, None, Lookup::Unseen),
        ] {
            let provider = Threaded::new(exhausted, as_of);
            provider.main.register_channel(&channel);
            let answers = provider
                .fetch_messages(&channel, None, &ids(&["404"]))
                .await
                .expect("read");
            assert_eq!(
                answers,
                std::slice::from_ref(&expected),
                "{exhausted} {as_of:?}"
            );
        }
    }

    #[test]
    fn a_provider_that_says_nothing_looks_up_by_id_as_it_always_did() {
        assert_eq!(ReadCaps::default().by_ids, Support::Emulated);
        assert_eq!(
            FakeDiscord::new()
                .read_caps(&ChannelId("42".to_owned()))
                .by_ids,
            Support::Emulated
        );
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use crate::discord::fake::FakeDiscord;

    const BASE_MS: i64 = 1_787_000_000_000;

    /// Ten messages a second apart, and the page requests the fake was sent.
    fn channel() -> (FakeDiscord, ChannelId) {
        let fake = FakeDiscord::new();
        let channel = ChannelId("42".to_owned());
        for i in 0..10 {
            fake.seed_at(&channel, "ada", &format!("m{i}"), BASE_MS + i * 1_000);
        }
        (fake, channel)
    }

    fn bodies(read: &RangeRead) -> Vec<&str> {
        read.messages.iter().map(|m| m.content.as_str()).collect()
    }

    #[tokio::test]
    async fn the_default_reads_one_page_from_the_windows_edge_and_trims_it_to_the_window() {
        let (fake, channel) = channel();
        let range = TimeRange {
            since_ms: BASE_MS + 2_000,
            until_ms: Some(BASE_MS + 5_000),
        };
        let read = fake
            .fetch_range(
                &channel,
                &ReadScope::Main,
                range,
                RangeOrder::OldestFirst,
                10,
                None,
            )
            .await
            .expect("read");
        assert_eq!(bodies(&read), ["m2", "m3", "m4"], "both edges to the ms");
        assert_eq!(fake.fetch_count(), 1);
        assert_eq!(read.next, None, "the page reached past the window's end");
        assert!(!read.exhausted, "a default never claims a proof");

        let read = fake
            .fetch_range(
                &channel,
                &ReadScope::Main,
                range,
                RangeOrder::NewestFirst,
                10,
                None,
            )
            .await
            .expect("read");
        assert_eq!(bodies(&read), ["m2", "m3", "m4"]);
        assert_eq!(read.next, None, "the page reached below the window's start");
        assert!(!read.exhausted);
    }

    #[tokio::test]
    async fn the_default_continues_from_the_last_message_a_step_reached() {
        let (fake, channel) = channel();
        let range = TimeRange {
            since_ms: BASE_MS + 1_000,
            until_ms: None,
        };
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let read = fake
                .fetch_range(
                    &channel,
                    &ReadScope::Main,
                    range,
                    RangeOrder::OldestFirst,
                    3,
                    cursor.as_deref(),
                )
                .await
                .expect("read");
            seen.extend(read.messages.iter().map(|m| m.content.clone()));
            cursor = read.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen, (1..10).map(|i| format!("m{i}")).collect::<Vec<_>>());

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let read = fake
                .fetch_range(
                    &channel,
                    &ReadScope::Main,
                    range,
                    RangeOrder::NewestFirst,
                    4,
                    cursor.as_deref(),
                )
                .await
                .expect("read");
            let mut step: Vec<String> = read.messages.iter().map(|m| m.content.clone()).collect();
            step.extend(seen);
            seen = step;
            cursor = read.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen, (1..10).map(|i| format!("m{i}")).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn the_default_refuses_a_scope_it_cannot_bound_by_time_and_sends_nothing() {
        let (fake, channel) = channel();
        let range = TimeRange {
            since_ms: BASE_MS,
            until_ms: None,
        };
        for scope in [ReadScope::Flat, ReadScope::Thread("t".to_owned())] {
            let error = fake
                .fetch_range(&channel, &scope, range, RangeOrder::OldestFirst, 5, None)
                .await
                .expect_err("refused");
            assert!(matches!(error, ChatError::Refused(_)), "{error}");
        }
        assert_eq!(fake.fetch_count(), 0);
        assert_eq!(fake.read_caps(&channel), ReadCaps::default());
        assert_eq!(fake.read_caps(&channel).range, Support::Emulated);
    }
}
