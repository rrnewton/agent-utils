//! The operations behind BOTH the JSON API and the MCP tools.
//!
//! There are now two front doors onto the same capability — `/api/v1/...` for the phone web app,
//! and `/mcp` for a hosted voice agent — and the dangerous failure mode is that they drift: one
//! of them forgets the channel allowlist, or the read/write split, or the untrusted-content
//! boundary. So the policy lives here, once, and both transports call it.
//!
//! What is deliberately NOT here: authentication. Deciding whether a caller may act at all is the
//! transport's job, because the two transports present credentials differently (a bearer header
//! on a REST route, a bearer header on a JSON-RPC POST) even though they check the same tokens.
//! Every function below assumes the caller has already been authorized for the scope it needs;
//! the scope each one needs is stated in its documentation and enforced by its only callers.

use crate::discord::DiscordError;
use crate::model::{ChannelId, ChannelInfo, Message, MessageId};
use crate::retrieval::{self, Resolution};
use crate::state::AppState;
use crate::summary::{self, DigestEntry, DEFAULT_SUMMARY_CHARS};

/// Why an operation could not be carried out.
///
/// These are the refusals a *correctly authenticated* caller can hit. Authentication failures do
/// not appear here at all — see [`crate::auth`].
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    /// The channel is not in the configured allowlist. This is the same answer a caller gets for
    /// a channel that does not exist, on purpose: the allowlist is not a directory to probe.
    #[error("that channel is not configured on this server")]
    UnknownChannel,
    /// The message is not in the window this server fetched.
    #[error("that message is not in the recent window for this channel")]
    UnknownMessage,
    /// A resolve call arrived with nothing to resolve.
    #[error("query must not be empty")]
    EmptyQuery,
    /// The channel is configured, but read-only.
    #[error("this channel is configured read-only")]
    ChannelNotWritable,
    /// A cursor argument was not a Discord message id.
    #[error("that cursor is not a message id; use the one the previous page handed back")]
    InvalidCursor,
    /// The requested span could not be turned into a single walk.
    #[error(
        "that is not a span this server can walk: give either a cursor to step back from, or a \
         start time and an optional end time, but not both"
    )]
    InvalidRange,
    /// Discord itself failed, or refused the request before it was sent.
    /// Some parts of a split message reached Discord and the rest did not.
    ///
    /// A DISTINCT variant rather than a `Discord` error, because the two call for opposite things
    /// from the caller. A plain failure means "nothing happened, send it again". This means "half
    /// of it is in the channel and cannot be unsent" — sending the whole thing again would post
    /// the first half twice. `unsent` is the remainder, and once the caller has cleared their
    /// draft it is the only copy of that text in existence, so it travels with the error rather
    /// than being reconstructable from it.
    #[error("posted {posted} part(s) and then failed: {cause}")]
    PartiallyPosted {
        /// How many parts Discord accepted before the failure.
        posted: usize,
        /// The text that did NOT reach Discord, ready to be sent on its own.
        unsent: String,
        /// Why the next part failed.
        cause: String,
    },

    /// Discord itself failed, or refused the request before it was sent.
    #[error(transparent)]
    Discord(#[from] DiscordError),
    /// The durable state store failed, or is not configured at all.
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
    /// The summariser could not produce a summary.
    #[error(transparent)]
    Summarizer(#[from] crate::summarize::SummaryError),
}

impl OpError {
    /// Stable machine-readable code, shared by both transports so a client can branch on it.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownChannel => "unknown_channel",
            Self::UnknownMessage => "unknown_message",
            Self::EmptyQuery => "empty_query",
            Self::ChannelNotWritable => "channel_not_writable",
            Self::InvalidCursor => "invalid_cursor",
            Self::InvalidRange => "invalid_range",
            Self::PartiallyPosted { .. } => "partially_posted",
            Self::Discord(DiscordError::Refused(_)) => "refused",
            Self::Discord(_) => "discord_error",
            Self::Store(inner) => inner.code(),
            Self::Summarizer(inner) => inner.code(),
        }
    }
}

/// Resolve a caller-supplied channel id against the allowlist.
///
/// This is the single gate every channel-scoped operation goes through. A snowflake that is not
/// configured does not exist here, however many channels the bot is actually in.
async fn allowed(state: &AppState, channel_id: &str) -> Result<ChannelInfo, OpError> {
    let mut channel = configured(state, channel_id)?;
    channel.alias = aliases(state).await.remove(channel.id.as_str());
    Ok(channel)
}

/// The allowlist gate WITHOUT the alias overlay.
///
/// Two callers, and both of them have a reason not to pay for a store read: [`watch`], which is
/// deliberately synchronous and throws the channel away, and the two alias operations themselves,
/// which are about to learn the alias from the write they are making. Everything that RENDERS a
/// channel name goes through [`allowed`] instead.
fn configured(state: &AppState, channel_id: &str) -> Result<ChannelInfo, OpError> {
    state
        .channel(channel_id)
        .cloned()
        .ok_or(OpError::UnknownChannel)
}

/// The channels this server is configured for, wearing the operator's own names. Read scope.
pub async fn channels(state: &AppState) -> Vec<ChannelInfo> {
    let aliases = aliases(state).await;
    state
        .config
        .channels
        .iter()
        .cloned()
        .map(|mut channel| {
            channel.alias = aliases.get(channel.id.as_str()).cloned();
            channel
        })
        .collect()
}

/// The operator's local channel names, by channel id. `#39 channel-alias`.
///
/// **A failure here is not a failure of the operation asking.** An alias is a decoration on a
/// channel, so a store that is not configured — the ordinary state of a deployment that has never
/// set `storage.path` — must not make reading a channel impossible; it degrades to the configured
/// label, which is exactly what the reader saw before there were aliases at all. A real backend
/// failure degrades the same way and says so in the log, because the alternative is that a
/// corrupt row takes the whole bridge down.
async fn aliases(state: &AppState) -> std::collections::BTreeMap<String, String> {
    match state.store.channel_aliases().await {
        Ok(found) => found
            .into_iter()
            .map(|a| (a.channel.as_str().to_owned(), a.alias))
            .collect(),
        // Expected whenever no store is configured. Not worth a line per request.
        Err(crate::store::StoreError::Unavailable(_)) => std::collections::BTreeMap::new(),
        Err(error) => {
            tracing::warn!(
                %error,
                "could not read channel aliases; falling back to the configured labels"
            );
            std::collections::BTreeMap::new()
        }
    }
}

/// Give one channel a local name. Write scope, and the OPERATOR's act alone.
///
/// Nothing about this reaches Discord: the channel keeps whatever name it has there, and no
/// caller outside this deployment can see the one set here. It is deliberately absent from the
/// MCP tool manifest — a model that could rename the channels it is also reporting on could
/// quietly make its own reports unrecognisable.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] outside the allowlist, so a snowflake nobody configured cannot be
/// used to grow the store one row at a time; [`OpError::Store`] when the text is unusable, when
/// no store is configured, or when the write fails.
pub async fn set_channel_alias(
    state: &AppState,
    channel_id: &str,
    alias: &str,
) -> Result<ChannelInfo, OpError> {
    let mut channel = configured(state, channel_id)?;
    let stored = state.store.set_channel_alias(&channel.id, alias).await?;
    channel.alias = Some(stored.alias);
    Ok(channel)
}

/// Drop one channel's local name, putting the configured label back. Write scope.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] outside the allowlist; [`OpError::Store`] when the channel had no
/// alias, when no store is configured, or when the write fails.
pub async fn clear_channel_alias(
    state: &AppState,
    channel_id: &str,
) -> Result<ChannelInfo, OpError> {
    let channel = configured(state, channel_id)?;
    state.store.clear_channel_alias(&channel.id).await?;
    Ok(channel)
}

/// Fill in [`Message::spoken_time`] for a freshly fetched batch.
///
/// THE ONLY PLACE the zone conversion happens. It sits here rather than at the render sites
/// because there are four of those — [`crate::untrusted::render_for_model`] and three handlers in
/// [`crate::mcp::protocol`] — and both front doors already funnel through this module. Formatting
/// per render site would mean writing the conversion four times and having it drift three ways.
///
/// Everything downstream PRINTS this field and must never compute it.
fn stamp(state: &AppState, messages: &mut [Message]) {
    for message in messages {
        message.spoken_time = crate::clock::spoken(&message.timestamp, &state.config.timezone);
    }
}

/// One fetched window of a channel, and what it took to fetch it.
///
/// The `limit` is carried alongside the messages for one reason: without it, `messages.len()` is
/// an unlabelled number that a caller will render as a channel total. It is not one. Discord's
/// REST API offers no message count for a guild text channel — `GET /channels/{id}/messages`
/// accepts `limit` 1..=100 and returns no total, and `message_count` / `total_message_sent` exist
/// only on THREAD channels — so the only thing this server can honestly say about a full window
/// is "there are at least this many".
#[derive(Clone, Debug)]
pub struct Window {
    /// Channel that was read.
    pub channel: ChannelInfo,
    /// Messages, oldest first.
    pub messages: Vec<Message>,
    /// The size actually asked of Discord, already clamped to what Discord will honour.
    pub limit: u16,
}

impl Window {
    /// Whether `messages` is the WHOLE channel rather than a window onto it.
    ///
    /// A fetch that comes back with fewer messages than it asked for is Discord saying there is
    /// nothing older, so the count is the channel's. A fetch that comes back full says nothing
    /// about how much more there is. This is the only case in which a precise number may be shown.
    #[must_use]
    pub fn is_whole_channel(&self) -> bool {
        self.messages.len() < usize::from(self.limit)
    }
}

/// Recent messages of one channel, oldest first. Read scope.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] for a channel outside the allowlist; [`OpError::Discord`] when the
/// fetch fails.
pub async fn messages(
    state: &AppState,
    channel_id: &str,
    limit: Option<u16>,
) -> Result<Window, OpError> {
    let channel = allowed(state, channel_id).await?;
    // Clamped to Discord's own ceiling, not merely to the configured one. Nothing in
    // `Config::from_toml_and_env` stops an operator writing `max_fetch_limit = 200`, but both the
    // real client and the fake clamp the request to 100 — so comparing against the unclamped
    // number would report a FULL 100-message window as the whole channel.
    let limit = state
        .effective_limit(limit)
        .min(crate::discord::http::DISCORD_MAX_LIMIT);
    let mut messages = state.discord.fetch_recent(&channel.id, limit).await?;
    stamp(state, &mut messages);
    Ok(Window {
        channel,
        messages,
        limit,
    })
}

/// One message in full, by id, within the recent window. Read scope.
///
/// # Errors
///
/// [`OpError::UnknownChannel`], [`OpError::UnknownMessage`] when the id is not in the fetched
/// window, or [`OpError::Discord`].
pub async fn message_by_id(
    state: &AppState,
    channel_id: &str,
    message_id: &str,
    limit: Option<u16>,
) -> Result<(ChannelInfo, Message), OpError> {
    let window = messages(state, channel_id, limit).await?;
    let wanted = MessageId(message_id.to_owned());
    let message = window
        .messages
        .into_iter()
        .find(|m| m.id == wanted)
        .ok_or(OpError::UnknownMessage)?;
    Ok((window.channel, message))
}

/// One speakable line per recent message. Read scope.
///
/// The trailing flag is [`Window::is_whole_channel`]: true when the entries are the entire
/// channel, false when they are a window onto something larger. A caller that renders a count
/// without consulting it is reporting the fetch size as a channel total.
///
/// # Errors
///
/// As [`messages`].
pub async fn digest(
    state: &AppState,
    channel_id: &str,
    limit: Option<u16>,
    width: Option<u16>,
) -> Result<(ChannelInfo, Vec<DigestEntry>, bool), OpError> {
    let window = messages(state, channel_id, limit).await?;
    let complete = window.is_whole_channel();
    let width = usize::from(width.unwrap_or(0));
    let width = if width == 0 {
        DEFAULT_SUMMARY_CHARS
    } else {
        width
    };
    Ok((
        window.channel,
        summary::digest(&window.messages, width),
        complete,
    ))
}

/// The largest page this server will hand back in one step.
///
/// One less than Discord's own ceiling, and that is the whole point: the page is fetched with
/// `limit + 1` and the extra message is dropped, which is the only way to answer "is there more?"
/// exactly rather than by guessing from a full window. Allowing 100 would make the probe
/// impossible on the largest page and force `has_more` back into a guess.
pub const MAX_PAGE: u16 = crate::discord::http::DISCORD_MAX_LIMIT - 1;

/// What a caller wants one step of a walk to cover.
///
/// Two modes, and mixing them is an error rather than a precedence rule: `before` steps BACKWARDS
/// from a cursor (or from the newest message when absent), and `since`/`until` jump to a
/// half-open time span. A caller that supplied both would have no way to know which it got.
#[derive(Clone, Copy, Debug, Default)]
pub struct PageRequest<'a> {
    /// How many messages to return. Clamped by the configured ceiling and by [`MAX_PAGE`].
    pub limit: Option<u16>,
    /// Step back from this message id, exclusive. Hand back [`Page::next_before`].
    pub before: Option<&'a str>,
    /// Start of a time span, inclusive, as an ISO-8601 instant.
    pub since: Option<&'a str>,
    /// End of a time span, EXCLUSIVE, as an ISO-8601 instant. Requires `since`.
    pub until: Option<&'a str>,
}

/// One step of a walk through a channel, and everything needed to take the next one.
///
/// Every field here exists to stop a caller mistaking a page for the whole. The issue this comes
/// from was reported because a model asked "how many messages are in this channel?" and answered
/// **100** — the size of the page it had been handed, by a response that never said it was a page.
#[derive(Clone, Debug)]
pub struct Page {
    /// Channel that was read.
    pub channel: ChannelInfo,
    /// The messages, oldest first.
    pub messages: Vec<Message>,
    /// The page size actually used, after both ceilings.
    pub limit: u16,
    /// Whether messages exist beyond this page in the direction of travel.
    ///
    /// Decided by over-fetching one and dropping it, so it is a fact rather than an inference from
    /// a full window.
    pub has_more: bool,
    /// The `before` cursor for the next step back. Set only when [`Page::has_more`].
    pub next_before: Option<MessageId>,
    /// The `since` instant for the next step of a range walk. Set only when [`Page::has_more`].
    pub next_since: Option<String>,
}

impl Page {
    /// How many messages this page carries.
    #[must_use]
    pub fn returned(&self) -> usize {
        self.messages.len()
    }
}

/// Parse a caller-supplied cursor into a message id.
fn cursor(value: Option<&str>) -> Result<Option<MessageId>, OpError> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(raw) => {
            let id = MessageId(raw.to_owned());
            if id.numeric().is_none() {
                return Err(OpError::InvalidCursor);
            }
            Ok(Some(id))
        }
    }
}

fn instant(value: Option<&str>) -> Result<Option<i64>, OpError> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(raw) => crate::clock::instant_ms(raw)
            .map(Some)
            .ok_or(OpError::InvalidRange),
    }
}

/// One step of a walk: a page that knows it is a page. Read scope.
///
/// # Errors
///
/// [`OpError::UnknownChannel`], [`OpError::InvalidCursor`] for a cursor that is not a snowflake,
/// [`OpError::InvalidRange`] for an unparseable or contradictory span, or [`OpError::Discord`].
pub async fn page(
    state: &AppState,
    channel_id: &str,
    request: PageRequest<'_>,
) -> Result<Page, OpError> {
    let channel = allowed(state, channel_id).await?;
    let before = cursor(request.before)?;
    let since = instant(request.since)?;
    let until = instant(request.until)?;
    if (since.is_some() || until.is_some()) && before.is_some() {
        return Err(OpError::InvalidRange);
    }
    if until.is_some() && since.is_none() {
        // An open-ended "everything before this instant" is a `before` walk wearing a time
        // costume, and answering it would need a cursor this server has no way to place a page
        // relative to. Say so instead of returning a span that is not the one asked for.
        return Err(OpError::InvalidRange);
    }
    if let (Some(from), Some(to)) = (since, until) {
        if to <= from {
            return Err(OpError::InvalidRange);
        }
    }

    let limit = state
        .effective_limit(request.limit)
        .min(MAX_PAGE)
        .min(state.config.discord.max_fetch_limit);
    // The over-fetch. One extra message answers "is there more" outright; the alternative is a
    // second round trip or the guess that a full window means more exists.
    let probe = limit + 1;

    let (mut messages, forward) = match since {
        Some(from) => {
            // Discord's `after` is EXCLUSIVE, and the `since` edge is inclusive, so the cursor
            // goes one millisecond earlier. Off by one here silently drops a message that sits
            // exactly on the boundary — the edge the issue asks be tested.
            let after = MessageId::at_time_ms(from - 1);
            let mut fetched = state
                .discord
                .fetch_page(&channel.id, probe, None, Some(&after))
                .await?;
            // Both edges applied to the message's own creation instant, because the boundary
            // cursor is only millisecond-accurate: the extra millisecond it lets through at the
            // bottom is trimmed here, and a message exactly at `until` is deterministically OUT.
            fetched.retain(|m| {
                message_ms(m).is_none_or(|ms| ms >= from && until.is_none_or(|to| ms < to))
            });
            (fetched, true)
        }
        None => {
            let fetched = state
                .discord
                .fetch_page(&channel.id, probe, before.as_ref(), None)
                .await?;
            (fetched, false)
        }
    };

    let has_more = messages.len() > usize::from(limit);
    if has_more {
        if forward {
            // Walking forward, the extra one is the NEWEST; drop it from that end.
            messages.truncate(usize::from(limit));
        } else {
            let excess = messages.len() - usize::from(limit);
            messages.drain(..excess);
        }
    }
    stamp(state, &mut messages);

    let (next_before, next_since) = if !has_more {
        (None, None)
    } else if forward {
        // Resume one millisecond after the newest message returned, so the next step covers the
        // rest of the span without repeating this one's last message.
        let next = messages
            .last()
            .and_then(message_ms)
            .map(|ms| crate::clock::iso_from_ms(ms + 1));
        (None, next)
    } else {
        (messages.first().map(|m| m.id.clone()), None)
    };

    Ok(Page {
        channel,
        messages,
        limit,
        has_more,
        next_before,
        next_since,
    })
}

/// When a message was created, from its snowflake, falling back to its reported timestamp.
///
/// The snowflake is authoritative because it is what Discord's own `before`/`after` compare, so a
/// range filtered by anything else could disagree with the page it was fetched from.
fn message_ms(message: &Message) -> Option<i64> {
    message
        .id
        .created_at_ms()
        .or_else(|| crate::clock::instant_ms(&message.timestamp))
}

/// A bounded, honest answer to "how many messages are in there?".
///
/// Discord publishes no message count for a guild text channel, so a promised total would be
/// either slow or invented. This walks backwards until the channel runs out or the cap stops it,
/// and says which happened.
#[derive(Clone, Debug)]
pub struct MessageCount {
    /// Channel that was counted.
    pub channel: ChannelInfo,
    /// How many messages were actually seen.
    pub counted: usize,
    /// Whether the cap stopped the walk, making `counted` a LOWER BOUND rather than a total.
    pub at_least: bool,
    /// The ceiling that applied.
    pub cap: u32,
    /// Oldest message reached, so a caller can carry on from there if it wants to.
    pub oldest_seen: Option<MessageId>,
    /// Newest message the walk started from.
    pub newest_seen: Option<MessageId>,
}

/// Count the messages in a channel, up to a cap, optionally only since an instant. Read scope.
///
/// # Errors
///
/// [`OpError::UnknownChannel`], [`OpError::InvalidRange`] for an unparseable `since`, or
/// [`OpError::Discord`].
pub async fn count(
    state: &AppState,
    channel_id: &str,
    since: Option<&str>,
    cap: Option<u32>,
) -> Result<MessageCount, OpError> {
    let channel = allowed(state, channel_id).await?;
    let since = instant(since)?;
    let cap = state.effective_count_cap(cap);

    let stride = crate::discord::http::DISCORD_MAX_LIMIT;
    let mut counted: usize = 0;
    let mut at_least = false;
    let mut oldest_seen: Option<MessageId> = None;
    let mut newest_seen: Option<MessageId> = None;
    let mut before: Option<MessageId> = None;

    loop {
        let batch = state
            .discord
            .fetch_page(&channel.id, stride, before.as_ref(), None)
            .await?;
        if batch.is_empty() {
            break;
        }
        if newest_seen.is_none() {
            newest_seen = batch.last().map(|m| m.id.clone());
        }
        // Oldest first, so the front of the batch is the far end of the walk.
        let oldest = batch.first().expect("non-empty").clone();
        let exhausted = batch.len() < usize::from(stride);

        let kept = match since {
            None => batch.len(),
            Some(from) => batch
                .iter()
                .filter(|m| message_ms(m).is_none_or(|ms| ms >= from))
                .count(),
        };
        counted += kept;
        oldest_seen = Some(oldest.id.clone());

        // Stop as soon as the walk has passed the `since` boundary: everything older is outside
        // the question, and continuing would spend requests to count nothing.
        let passed_since = since.is_some() && kept < batch.len();
        if passed_since || exhausted {
            break;
        }
        if counted >= usize::try_from(cap).unwrap_or(usize::MAX) {
            at_least = true;
            break;
        }
        before = Some(oldest.id);
    }

    Ok(MessageCount {
        channel,
        counted,
        at_least,
        cap,
        oldest_seen,
        newest_seen,
    })
}

/// Semantic random access: describe a message, get that message. Read scope.
///
/// Returns the resolution and how many messages were searched, so a caller can honestly say "I
/// only looked at the last twenty".
///
/// # Errors
///
/// [`OpError::EmptyQuery`] for a blank query, plus the errors of [`messages`].
pub async fn resolve(
    state: &AppState,
    channel_id: &str,
    query: &str,
    limit: Option<u16>,
    max_alternatives: Option<u16>,
) -> Result<(ChannelInfo, Resolution, usize), OpError> {
    let info = allowed(state, channel_id).await?;
    if query.trim().is_empty() {
        return Err(OpError::EmptyQuery);
    }
    let limit = state.effective_limit(limit);
    // This path does NOT go through `messages` above — it fetches directly — so it needs its own
    // stamping call. `run_find` renders a timestamp for the best match and every alternative, and
    // without this those lines would be the only ones still speaking raw UTC.
    let mut messages = state.discord.fetch_recent(&info.id, limit).await?;
    stamp(state, &mut messages);
    let max_alternatives = usize::from(max_alternatives.unwrap_or(3)).min(10);
    let resolution = retrieval::resolve(state.ranker.as_ref(), &messages, query, max_alternatives);
    let searched = messages.len();
    Ok((info, resolution, searched))
}

/// Post a message as the bot. **Write scope**, and the only operation that speaks in the owner's
/// name.
///
/// Two fences, in this order: the channel must be in the allowlist at all, and it must
/// additionally be marked writable. A channel that is merely configured is not postable.
///
/// # Errors
///
/// [`OpError::UnknownChannel`], [`OpError::ChannelNotWritable`], or [`OpError::Discord`] — which
/// also covers the length and emptiness refusals the Discord layer makes before any request is
/// sent.
pub async fn reply(
    state: &AppState,
    channel_id: &str,
    text: &str,
    reply_to: Option<&str>,
) -> Result<(ChannelInfo, Message, Vec<Message>), OpError> {
    let info = allowed(state, channel_id).await?;
    if !info.writable {
        return Err(OpError::ChannelNotWritable);
    }
    let reply_to = reply_to.map(|id| MessageId(id.to_owned()));

    // LONGER THAN DISCORD ACCEPTS IS NOT A REFUSAL ANY MORE. A reply somebody typed on a phone came
    // back as an error and stayed stuck in the box; coding agents posting into the channel already
    // split their own long messages. See `crate::discord::split`.
    let parts = crate::discord::split::split_for_discord(text);
    if parts.is_empty() {
        return Err(OpError::Discord(DiscordError::Refused(
            "message content is empty".to_owned(),
        )));
    }

    let mut posted: Vec<Message> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        // Only the FIRST part answers the message. Discord threads a reply from one message, and
        // pointing every part at the same parent would render as several separate answers to the
        // same thing rather than as one answer that ran long.
        let parent = if index == 0 { reply_to.as_ref() } else { None };
        match state.discord.post_message(&info.id, part, parent).await {
            Ok(mut one) => {
                stamp(state, std::slice::from_mut(&mut one));
                // Remember that WE posted this, before anything can observe it. `#44 live-push`:
                // the poller will see it in a few seconds, and relaying it into the live
                // conversation would make the agent hear its own reply as new information.
                state.live.note_self_posted(&one.id);
                posted.push(one);
            }
            Err(cause) => {
                // PART OF IT IS ALREADY IN THE CHANNEL and cannot be unsent. Reporting a plain
                // failure here would be a lie the reader acts on: they would send the whole thing
                // again and post the first half twice. So the caller is told exactly how much
                // landed AND handed back the text that did not, which is the only copy of it left
                // once their draft is cleared.
                let unsent: String = parts[index..].join("");
                return Err(OpError::PartiallyPosted {
                    posted: posted.len(),
                    unsent,
                    cause: cause.to_string(),
                });
            }
        }
    }
    let first = posted.first().cloned().expect("at least one part posted");
    Ok((info, first, posted))
}

/// Attach to a channel's live feed. Read scope.
///
/// The reason this is here rather than in the handler is the allowlist. [`allowed`] is private to
/// this module on purpose — the module doc says both front doors go through it — and a stream is
/// the easiest thing on this server to leave accidentally open, so it goes through the same gate
/// as a fetch and gets the same refusal for a channel outside the configuration. Never a 200 with
/// an empty stream: that reads as "this channel is quiet" rather than "there is no such channel".
///
/// `after` is a resume point, taken from the caller's `Last-Event-ID`.
///
/// Not `async`, unlike everything else here: subscribing touches no network and no store, and
/// making it await nothing would be a promise about future cost that this does not have.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] for a channel outside the allowlist.
pub fn watch(
    state: &AppState,
    channel_id: &str,
    after: Option<&MessageId>,
) -> Result<(ChannelInfo, crate::live::Subscription), OpError> {
    let channel = configured(state, channel_id)?;
    let subscription = state.live.subscribe(&channel.id, after);
    Ok((channel, subscription))
}

/// One channel's inbox state, as far as THIS SERVER is concerned.
///
/// See [`crate::store::INBOX_NOTICE`]: nothing here comes from Discord and nothing here goes back
/// to it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct InboxEntry {
    /// The channel, as configured here.
    pub channel: ChannelInfo,
    /// The newest message the owner has been shown, when this server has been told.
    pub last_read: Option<MessageId>,
    /// When the mark was set, in milliseconds since the Unix epoch.
    pub marked_at_ms: Option<i64>,
}

/// Inbox state for every configured channel, in configuration order. Read scope.
///
/// Every configured channel appears, including the ones with no mark: "never marked" is a state
/// the interface has to be able to show, and an absent row would read as "no such channel".
///
/// # Errors
///
/// [`OpError::Store`] when no store is configured, or the read fails.
pub async fn inbox(state: &AppState) -> Result<Vec<InboxEntry>, OpError> {
    let marks = state.store.read_marks().await?;
    Ok(state
        .config
        .channels
        .iter()
        .map(|channel| {
            let mark = marks.iter().find(|m| m.channel == channel.id);
            InboxEntry {
                channel: channel.clone(),
                last_read: mark.map(|m| m.last_read.clone()),
                marked_at_ms: mark.map(|m| m.marked_at_ms),
            }
        })
        .collect())
}

/// Move this server's read mark for one channel forward. Write scope.
///
/// **Write scope, and the reason is not that it touches Discord** — it does not, and cannot. It
/// mutates durable server state that another device will read back, which is a strictly larger
/// capability than any of the reads on this server, and the `/voice` page already holds the write
/// token.
///
/// Goes through [`allowed`] like every other channel operation, so a snowflake outside the
/// configuration cannot be used to grow the store one row at a time.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] outside the allowlist; [`OpError::Store`] when no store is
/// configured, the id cannot be ordered, or the write fails.
pub async fn mark_read(
    state: &AppState,
    channel_id: &str,
    upto: &MessageId,
) -> Result<(ChannelInfo, crate::store::ReadMark), OpError> {
    let channel = allowed(state, channel_id).await?;
    let mark = state.store.mark_read(&channel.id, upto).await?;
    Ok((channel, mark))
}

/// Drop this server's read mark for one channel, making its whole window unread again. Write
/// scope.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] outside the allowlist; [`OpError::Store`] when there was no mark,
/// when no store is configured, or when the write fails.
pub async fn forget_read_mark(state: &AppState, channel_id: &str) -> Result<ChannelInfo, OpError> {
    let channel = allowed(state, channel_id).await?;
    state.store.forget_read_mark(&channel.id).await?;
    Ok(channel)
}

// --- the to-do view: which messages have NOT been dealt with -------------------------------------
//
// `#50 todo-view`. A long backlog of assistant messages is a to-do list in practice, and Discord
// has nowhere to write down which of them you have finished with. So this server writes it down —
// see `#61 unread-status` and the module documentation on `crate::store`: the overlay is OURS,
// there is no sync-in and no sync-back, and every answer here says so out loud rather than letting
// the owner find it from a divergence.
//
// WHAT IS NOT HERE, stated so a reader of this file does not go looking for it. `#50` also asks
// for a message to leave the list when it is REPLIED to, detected from the reply reference Discord
// records. That needs a field on `crate::model::Message` and a conversion in `crate::discord`, and
// it is deliberately a separate change: it is a wire-format change touching every struct literal
// that builds a Message, and landing it inside this one would make an overlay nobody could review
// on its own. Until it lands, "dealt with" is DECLARED and never derived — which is also why the
// question of which wins when the two disagree does not arise yet.

/// One channel's to-do list: what has not been dealt with, and how much has.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TodoView {
    /// The channel, as configured here.
    pub channel: ChannelInfo,
    /// The messages still wanting attention, oldest first.
    pub messages: Vec<Message>,
    /// How many messages the window held before the dealt-with ones were dropped.
    ///
    /// Reported so a caller can say "9 of 30 left" rather than implying the channel holds nine
    /// messages. It is the WINDOW's size, not the channel's: see [`Window::is_whole_channel`].
    pub window: usize,
    /// Whether that window is the whole channel, so a caller can avoid a confident wrong total.
    pub complete: bool,
}

/// The messages in one channel that have not been dealt with here. Read scope to ask.
///
/// The list is the recent window MINUS the dismissals, and nothing else: there is no server-side
/// notion of "important" and no inference from dwell time, both of which `#50` excludes
/// deliberately. A message leaves this list because somebody said so.
///
/// A store that is not configured is an ERROR here, unlike in [`summarize_message`], and the
/// difference is not an inconsistency. There, the store is a cache and degrading to "generate
/// every time" produces the same answer more slowly. Here it is the ANSWER: with no overlay every
/// message is undealt-with, so a to-do list served from a missing store would silently be the
/// plain channel wearing a name that promises filtering.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] outside the allowlist; [`OpError::Discord`] when the fetch fails;
/// [`OpError::Store`] when no store is configured or the read fails.
pub async fn todo(
    state: &AppState,
    channel_id: &str,
    limit: Option<u16>,
) -> Result<TodoView, OpError> {
    let window = messages(state, channel_id, limit).await?;
    let complete = window.is_whole_channel();
    let dismissed = state.store.dismissals(&window.channel.id).await?;
    let dismissed: std::collections::HashSet<&str> =
        dismissed.iter().map(MessageId::as_str).collect();
    let total = window.messages.len();
    let left = window
        .messages
        .into_iter()
        .filter(|message| !dismissed.contains(message.id.as_str()))
        .collect();
    Ok(TodoView {
        channel: window.channel,
        messages: left,
        window: total,
        complete,
    })
}

/// Which of `messages` the reader has archived.
///
/// Intersected with the window rather than returned whole: the store holds every dismissal the
/// channel has ever had, and a client only ever needs to grey the rows it is showing. Sending the
/// lot would grow without bound and say nothing the page could use.
///
/// `#50 todo-view`. The To do filter REMOVES these; the ordinary channel view dims them, and until
/// this existed it could not, because nothing in the messages payload said which ones they were.
///
/// # Errors
///
/// [`OpError`] when the store cannot be read.
pub async fn dismissed_within(
    state: &AppState,
    channel: &ChannelId,
    messages: &[Message],
) -> Result<Vec<MessageId>, OpError> {
    let present: std::collections::HashSet<&str> =
        messages.iter().map(|message| message.id.as_str()).collect();
    let dismissed = state.store.dismissals(channel).await?;
    Ok(dismissed
        .into_iter()
        .filter(|id| present.contains(id.as_str()))
        .collect())
}

/// What one dismissal or one restoration did.
#[derive(Clone, Debug, serde::Serialize)]
pub struct InboxChange {
    /// The channel, as configured here.
    pub channel: ChannelInfo,
    /// Exactly which messages changed state, oldest first.
    ///
    /// The list, not merely a count, and that is what makes the undo exact: a bulk clear reports
    /// the set it cleared, and putting that set back restores precisely what went — not "the last
    /// N", which is a different set the moment anything else has happened.
    pub messages: Vec<MessageId>,
}

/// Mark messages as dealt with here. Write scope.
///
/// Every id is checked against the fetched window first, so this cannot be used to grow the store
/// one invented snowflake at a time — the same reason [`allowed`] guards the channel. An id that
/// is not in the window is a refusal for the WHOLE batch rather than a silent omission: a partial
/// bulk action that reported success would leave the caller's undo describing a set that was
/// never dismissed.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] outside the allowlist; [`OpError::UnknownMessage`] when an id is
/// not in the window; [`OpError::Discord`]; [`OpError::Store`].
pub async fn dismiss(
    state: &AppState,
    channel_id: &str,
    wanted: &[MessageId],
) -> Result<InboxChange, OpError> {
    let window = messages(state, channel_id, None).await?;
    let known: std::collections::HashSet<&str> =
        window.messages.iter().map(|m| m.id.as_str()).collect();
    if wanted.iter().any(|id| !known.contains(id.as_str())) {
        return Err(OpError::UnknownMessage);
    }
    state.store.dismiss(&window.channel.id, wanted).await?;
    Ok(InboxChange {
        channel: window.channel,
        messages: wanted.to_vec(),
    })
}

/// Declare bankruptcy: mark everything in the window at or before `through` as dealt with.
///
/// **The boundary is INCLUDED.** "Everything before this one" is what a person means when they
/// press on a row and give up on the backlog, and the row they pressed is part of what they are
/// giving up on. That is a decision, not an accident, and it is tested at the boundary.
///
/// **`limit` is the window the caller READ with, and it bounds what this can clear.** The same
/// number that was passed to [`todo`], because "everything before this one" means everything the
/// reader was looking at — and a caller that read a three-message page must not be able to clear a
/// fifty-message backlog by naming the oldest row of the three. Passing `None` means the default
/// window, which is what a caller that read `/todo` without a limit was shown. A `through` outside
/// the narrowed window is [`OpError::UnknownMessage`], which is the right refusal: the caller is
/// naming a boundary it did not display.
///
/// Contrast [`dismiss`], which takes no window: there the window is only a check that the named
/// ids exist, so narrowing it could only refuse an id the caller really did see.
///
/// Answers the exact set it cleared, so the undo is exact — which is what makes a bulk,
/// destructive action safe enough to offer at all. Messages already dealt with are not in it: they
/// did not change, and restoring them would resurrect something the reader cleared earlier and
/// never asked to see again.
///
/// # Errors
///
/// As [`dismiss`]. `through` must itself be in the window `limit` describes.
pub async fn declare_bankruptcy(
    state: &AppState,
    channel_id: &str,
    through: &MessageId,
    limit: Option<u16>,
) -> Result<InboxChange, OpError> {
    let window = messages(state, channel_id, limit).await?;
    let boundary = window
        .messages
        .iter()
        .position(|m| &m.id == through)
        .ok_or(OpError::UnknownMessage)?;
    let already = state.store.dismissals(&window.channel.id).await?;
    let already: std::collections::HashSet<&str> = already.iter().map(MessageId::as_str).collect();
    // By POSITION in the window, which is already sorted oldest-first, rather than by comparing
    // snowflakes here: the window is the ordering this server has, and a second ordering rule
    // beside it is a second thing to get wrong.
    let clearing: Vec<MessageId> = window.messages[..=boundary]
        .iter()
        .map(|m| m.id.clone())
        .filter(|id| !already.contains(id.as_str()))
        .collect();
    state.store.dismiss(&window.channel.id, &clearing).await?;
    Ok(InboxChange {
        channel: window.channel,
        messages: clearing,
    })
}

/// Put messages back into the to-do list. Write scope.
///
/// The undo. Not checked against the window, on purpose and unlike [`dismiss`]: this only ever
/// REMOVES rows, so an id that is not there costs nothing, and refusing a set because one of its
/// messages has since scrolled out of the window would make an undo fail exactly when the reader
/// most needs it.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] outside the allowlist; [`OpError::Store`].
pub async fn restore(
    state: &AppState,
    channel_id: &str,
    wanted: &[MessageId],
) -> Result<InboxChange, OpError> {
    let channel = allowed(state, channel_id).await?;
    state.store.restore(&channel.id, wanted).await?;
    Ok(InboxChange {
        channel,
        messages: wanted.to_vec(),
    })
}

/// What happened when a summary was asked for.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "summary")]
pub enum SummaryOutcome {
    /// The message was short enough to read as it is; nothing was generated and nothing was
    /// cached. Distinguished from a summary on purpose: a page must be able to show the original
    /// rather than a shortened copy of something that was already short.
    BelowThreshold,
    /// Served from the store, under the current policy.
    Cached(String),
    /// Generated now, and filed.
    Generated(String),
}

impl SummaryOutcome {
    /// The summary text, when there is one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::BelowThreshold => None,
            Self::Cached(text) | Self::Generated(text) => Some(text),
        }
    }
}

/// One summary, and what it cost to obtain.
///
/// A struct rather than a tuple because the third member is the whole point of it existing: the
/// open question about a hosted conversational agent is how long a round trip to one takes
/// compared with a full-size model, and a deployment that cannot answer that from its own logs and
/// its own API has to run a separate experiment to find out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summarised {
    /// The channel it came from.
    pub channel: ChannelInfo,
    /// Whether it was below the threshold, served from the cache, or generated now.
    pub outcome: SummaryOutcome,
    /// Wall-clock milliseconds spent inside the summariser.
    ///
    /// `None` when none was called — a message below the threshold, or a cache hit. That is the
    /// distinction worth keeping: a zero here would say "the vendor answered instantly", which is
    /// a very different claim from "no vendor was asked".
    pub generated_in_ms: Option<u64>,
}

/// One message, summarised — from the cache when the policy and the text are unchanged. Read
/// scope to ASK; write scope to leave anything behind.
///
/// The ORDER here is load-bearing and is the whole of the issue's first requirement: the
/// threshold is checked BEFORE the cache and before the summariser, so a short message costs
/// neither a lookup nor a call. Then the cache, then the summariser, then the write.
///
/// `caller` is the scope the credential actually carries, and it decides ONE thing: whether the
/// generated summary is filed. A read-scope token may SPEND the cache and may never FILL it. The
/// read token is the one pasted into a hosted voice agent, and a durable write reachable from the
/// least-trusted credential is precisely what the two-token split exists to prevent — see
/// [`crate::http`] for the rule stated once. A read-scope caller therefore gets the same answer,
/// generated every time, exactly as it would with no store configured at all.
///
/// The summariser is a model being fed channel text, so the request is built through
/// [`crate::summarize::SummaryRequest::new`], which frames it with [`crate::untrusted`] — the
/// fence is part of constructing the request rather than something this function could forget. A
/// summary is not exempt from the boundary for being short.
///
/// # Errors
///
/// [`OpError::UnknownChannel`] outside the allowlist; [`OpError::UnknownMessage`] when the
/// message is not in the fetched window; [`OpError::Discord`] when the fetch fails;
/// [`OpError::Summarizer`] when the summariser refuses. A store that is unavailable is NOT an
/// error here — see below.
pub async fn summarize_message(
    state: &AppState,
    caller: crate::auth::Scope,
    channel_id: &str,
    message_id: &str,
    limit: Option<u16>,
) -> Result<Summarised, OpError> {
    let window = messages(state, channel_id, limit).await?;
    let position = window
        .messages
        .iter()
        .position(|m| m.id.as_str() == message_id)
        .ok_or(OpError::UnknownMessage)?;
    let target = &window.messages[position];

    if target.content.chars().count() < state.config.summaries.threshold_chars {
        return Ok(Summarised {
            channel: window.channel.clone(),
            outcome: SummaryOutcome::BelowThreshold,
            generated_in_ms: None,
        });
    }

    let key = crate::store::SummaryKey {
        channel: window.channel.id.clone(),
        message: target.id.clone(),
        content_hash: crate::summarize::content_hash(&target.content),
        version: state.summary_version.to_string(),
    };
    // A store that is absent or broken degrades to "generate every time", loudly in the log and
    // silently to the caller. The alternative — refusing to summarise because the CACHE is not
    // configured — would make an optional durability feature a hard dependency of a read.
    match state.store.cached_summary(&key).await {
        Ok(Some(hit)) => {
            return Ok(Summarised {
                channel: window.channel.clone(),
                outcome: SummaryOutcome::Cached(hit),
                generated_in_ms: None,
            })
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "summary cache unreadable; generating uncached"),
    }

    let start = position.saturating_sub(state.config.summaries.context_messages);
    let context = &window.messages[start..position];
    let request =
        crate::summarize::SummaryRequest::new(target, context, state.config.summaries.target_chars);
    // Timed HERE rather than inside a backend, so the number means the same thing whichever
    // summariser is configured and a deployment can compare them against each other. The backend
    // is free to log a finer breakdown of its own; this is the one every backend reports.
    let began = std::time::Instant::now();
    let generated = state.summarizer.summarize(&request).await;
    let generated_in_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    // Logged on the failure path too. A summariser that takes thirty seconds to time out is a
    // performance fact about the deployment, and it is the one a person is most likely to want.
    tracing::info!(
        backend = state.summarizer.backend(),
        elapsed_ms = generated_in_ms,
        ok = generated.is_ok(),
        "asked the summariser for one message"
    );
    let generated = generated?;
    if caller >= crate::auth::Scope::Write {
        if let Err(error) = state.store.cache_summary(&key, &generated).await {
            tracing::warn!(%error, "summary could not be cached; it will be generated again");
        }
    } else {
        tracing::debug!(
            "a read-scope caller asked for a summary; it was generated and NOT filed, because a \
             read token may spend the cache and never fill it"
        );
    }
    Ok(Summarised {
        channel: window.channel,
        outcome: SummaryOutcome::Generated(generated),
        generated_in_ms: Some(generated_in_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StateStore as _;
    use crate::testing::{self, READ_CHANNEL, WRITE_CHANNEL};

    #[tokio::test]
    async fn a_channel_outside_the_allowlist_is_refused_for_reads() {
        let (state, _fake) = testing::state();
        let error = messages(&state, "9999999999", None)
            .await
            .expect_err("an unconfigured channel must not be readable");
        assert!(matches!(error, OpError::UnknownChannel), "{error:?}");
    }

    #[tokio::test]
    async fn a_channel_outside_the_allowlist_is_refused_for_writes_and_never_reaches_discord() {
        let (state, fake) = testing::state();
        let error = reply(&state, "9999999999", "hello", None)
            .await
            .expect_err("an unconfigured channel must not be postable");
        assert!(matches!(error, OpError::UnknownChannel), "{error:?}");
        assert!(
            fake.posted().is_empty(),
            "a refused post must not reach Discord at all: {:?}",
            fake.posted()
        );
    }

    #[tokio::test]
    async fn a_read_only_channel_is_refused_for_writes_and_never_reaches_discord() {
        let (state, fake) = testing::state();
        let error = reply(&state, READ_CHANNEL, "hello", None)
            .await
            .expect_err("a read-only channel must not be postable");
        assert!(matches!(error, OpError::ChannelNotWritable), "{error:?}");
        assert!(fake.posted().is_empty(), "{:?}", fake.posted());
    }

    #[tokio::test]
    async fn a_writable_channel_actually_posts_what_was_asked_for() {
        let (state, fake) = testing::state();
        let (info, posted, _parts) = reply(&state, WRITE_CHANNEL, "shipped it", None)
            .await
            .expect("the writable channel accepts a post");
        assert_eq!(info.id.as_str(), WRITE_CHANNEL);
        assert_eq!(posted.content, "shipped it");
        let recorded = fake.posted();
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0].channel.as_str(), WRITE_CHANNEL);
        assert_eq!(recorded[0].content, "shipped it");
    }

    #[tokio::test]
    async fn an_empty_query_is_refused_before_anything_is_fetched() {
        let (state, _fake) = testing::state();
        let error = resolve(&state, READ_CHANNEL, "   ", None, None)
            .await
            .expect_err("a blank query has nothing to resolve");
        assert!(matches!(error, OpError::EmptyQuery), "{error:?}");
    }

    #[tokio::test]
    async fn an_unknown_message_id_is_a_miss_not_a_substitute() {
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        fake.seed(&channel, "agent", "first");
        fake.seed(&channel, "agent", "second");
        let error = message_by_id(&state, READ_CHANNEL, "404040404040", None)
            .await
            .expect_err("an id outside the window must miss");
        assert!(matches!(error, OpError::UnknownMessage), "{error:?}");
    }

    #[tokio::test]
    async fn a_short_fetch_is_the_whole_channel_and_a_full_one_is_not() {
        // The fixture's default_fetch_limit is 20 (see `testing::config_toml`).
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        for i in 0..5 {
            fake.seed(&channel, "agent", &format!("line {i}"));
        }
        let window = messages(&state, READ_CHANNEL, None).await.expect("reads");
        assert!(
            window.is_whole_channel(),
            "a fetch that came back short is Discord saying there is nothing older"
        );

        for i in 0..40 {
            fake.seed(&channel, "agent", &format!("more {i}"));
        }
        let window = messages(&state, READ_CHANNEL, None).await.expect("reads");
        assert_eq!(window.messages.len(), 20);
        assert!(
            !window.is_whole_channel(),
            "a window that filled says nothing about how much more there is"
        );
    }

    #[tokio::test]
    async fn a_configured_limit_above_discords_ceiling_does_not_fake_a_whole_channel() {
        // The clamp trap. Nothing in configuration validation stops `max_fetch_limit = 200`, but
        // Discord tops out at 100 — so a 100-message answer to a "200" request is a FULL window,
        // not a small channel, and comparing against 200 would report the wrong thing.
        let toml = testing::config_toml()
            .replace("default_fetch_limit = 20", "default_fetch_limit = 200")
            .replace("max_fetch_limit = 50", "max_fetch_limit = 200");
        let (state, fake, _elevenlabs) = testing::state_from_toml(&toml);
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        for i in 0..120 {
            fake.seed(&channel, "agent", &format!("line {i}"));
        }
        let window = messages(&state, READ_CHANNEL, None).await.expect("reads");
        assert_eq!(
            window.limit,
            crate::discord::http::DISCORD_MAX_LIMIT,
            "the window must record the size Discord was actually asked for"
        );
        assert_eq!(window.messages.len(), 100);
        assert!(
            !window.is_whole_channel(),
            "a full 100-message window must never be reported as the whole channel"
        );
    }

    #[tokio::test]
    async fn a_page_knows_whether_more_remain_without_a_second_round_trip() {
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        for i in 0..25 {
            fake.seed(&channel, "agent", &format!("m{i}"));
        }
        let before = fake.fetch_count();
        let step = page(
            &state,
            READ_CHANNEL,
            PageRequest {
                limit: Some(10),
                ..PageRequest::default()
            },
        )
        .await
        .expect("reads");
        assert_eq!(
            step.returned(),
            10,
            "the extra probe message must be dropped"
        );
        assert!(step.has_more);
        assert_eq!(
            step.next_before.as_ref().map(MessageId::as_str),
            Some(step.messages[0].id.as_str()),
            "the cursor to step back from is the OLDEST message this page returned"
        );
        assert_eq!(
            fake.fetch_count() - before,
            1,
            "over-fetching by one is the point: answering \"is there more\" must not cost a \
             second request"
        );
        assert_eq!(
            step.messages.last().expect("non-empty").content,
            "m24",
            "an uncursored page starts at the newest message"
        );
    }

    #[tokio::test]
    async fn the_last_page_says_there_is_nothing_beyond_it() {
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        for i in 0..4 {
            fake.seed(&channel, "agent", &format!("m{i}"));
        }
        let step = page(
            &state,
            READ_CHANNEL,
            PageRequest {
                limit: Some(10),
                ..PageRequest::default()
            },
        )
        .await
        .expect("reads");
        assert_eq!(step.returned(), 4);
        assert!(
            !step.has_more,
            "a control: an implementation that always claimed more would pass the other test"
        );
        assert!(step.next_before.is_none());
    }

    #[tokio::test]
    async fn stepping_twice_covers_a_disjoint_span_with_nothing_skipped_at_the_boundary() {
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        for i in 0..25 {
            fake.seed(&channel, "agent", &format!("m{i}"));
        }
        let first = page(
            &state,
            READ_CHANNEL,
            PageRequest {
                limit: Some(10),
                ..PageRequest::default()
            },
        )
        .await
        .expect("reads");
        let cursor = first.next_before.clone().expect("more remain");
        let second = page(
            &state,
            READ_CHANNEL,
            PageRequest {
                limit: Some(10),
                before: Some(cursor.as_str()),
                ..PageRequest::default()
            },
        )
        .await
        .expect("reads");

        let ids = |p: &Page| p.messages.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        let (a, b) = (ids(&first), ids(&second));
        assert!(
            a.iter().all(|id| !b.contains(id)),
            "the two steps overlap: {a:?} / {b:?}"
        );
        let mut union: Vec<_> = b.iter().chain(a.iter()).cloned().collect();
        assert_eq!(union.len(), 20);
        let expected: Vec<_> = (5..25).map(|i| format!("m{i}")).collect();
        let got: Vec<_> = second
            .messages
            .iter()
            .chain(first.messages.iter())
            .map(|m| m.content.clone())
            .collect();
        assert_eq!(
            got, expected,
            "the two steps together must be exactly the newest twenty, in order, with nothing \
             skipped or repeated at the boundary"
        );
        union.dedup();
        assert_eq!(union.len(), 20);
    }

    #[tokio::test]
    async fn a_time_range_is_exact_at_both_edges() {
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        let base = 1_787_000_000_000_i64;
        for (offset, label) in [
            (-1_000, "before the window"),
            (0, "exactly at the start"),
            (5_000, "inside"),
            (10_000, "exactly at the end"),
            (15_000, "after the window"),
        ] {
            fake.seed_at(&channel, "agent", label, base + offset);
        }
        let since = crate::clock::iso_from_ms(base);
        let until = crate::clock::iso_from_ms(base + 10_000);
        let step = page(
            &state,
            READ_CHANNEL,
            PageRequest {
                limit: Some(10),
                since: Some(&since),
                until: Some(&until),
                ..PageRequest::default()
            },
        )
        .await
        .expect("reads");
        assert_eq!(
            step.messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["exactly at the start", "inside"],
            "the start edge is INCLUSIVE and the end edge is EXCLUSIVE, both exactly"
        );
        assert!(!step.has_more);
    }

    #[tokio::test]
    async fn a_range_walk_hands_back_where_to_resume() {
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        let base = 1_787_000_000_000_i64;
        for i in 0..6 {
            fake.seed_at(&channel, "agent", &format!("m{i}"), base + i * 1_000);
        }
        let since = crate::clock::iso_from_ms(base);
        let first = page(
            &state,
            READ_CHANNEL,
            PageRequest {
                limit: Some(2),
                since: Some(&since),
                ..PageRequest::default()
            },
        )
        .await
        .expect("reads");
        assert_eq!(
            first
                .messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["m0", "m1"],
            "a range walks FORWARD from its start, oldest first"
        );
        assert!(first.has_more);
        let resume = first.next_since.clone().expect("a place to resume");
        let second = page(
            &state,
            READ_CHANNEL,
            PageRequest {
                limit: Some(2),
                since: Some(&resume),
                ..PageRequest::default()
            },
        )
        .await
        .expect("reads");
        assert_eq!(
            second
                .messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["m2", "m3"],
            "resuming must continue rather than repeat the last message of the previous step"
        );
    }

    #[tokio::test]
    async fn a_page_refuses_arguments_it_cannot_honour_instead_of_picking_one() {
        let (state, _fake) = testing::state();
        let cases: [(PageRequest<'_>, &str); 4] = [
            (
                PageRequest {
                    before: Some("not-a-snowflake"),
                    ..PageRequest::default()
                },
                "invalid_cursor",
            ),
            (
                PageRequest {
                    since: Some("yesterday afternoon"),
                    ..PageRequest::default()
                },
                "invalid_range",
            ),
            (
                PageRequest {
                    before: Some("1000000000000000001"),
                    since: Some("2026-08-19T00:00:00Z"),
                    ..PageRequest::default()
                },
                "invalid_range",
            ),
            (
                PageRequest {
                    since: Some("2026-08-19T10:00:00Z"),
                    until: Some("2026-08-19T09:00:00Z"),
                    ..PageRequest::default()
                },
                "invalid_range",
            ),
        ];
        for (request, code) in cases {
            let error = page(&state, READ_CHANNEL, request)
                .await
                .expect_err("must refuse");
            assert_eq!(error.code(), code, "{request:?} -> {error}");
        }
    }

    #[tokio::test]
    async fn the_count_is_not_the_page_size() {
        // The reported bug, reproduced. The fixture's max_fetch_limit is 50 and Discord's own
        // page ceiling is 100; a channel of 150 must answer 150, never either of those.
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        for i in 0..150 {
            fake.seed(&channel, "agent", &format!("m{i}"));
        }
        let tally = count(&state, READ_CHANNEL, None, None)
            .await
            .expect("counts");
        assert_eq!(tally.counted, 150);
        assert!(
            !tally.at_least,
            "the walk reached the end, so this is a total and must not be hedged"
        );
        assert_ne!(tally.counted, 50);
        assert_ne!(tally.counted, 100);
    }

    #[tokio::test]
    async fn a_count_stopped_by_the_ceiling_says_it_is_a_lower_bound() {
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        for i in 0..350 {
            fake.seed(&channel, "agent", &format!("m{i}"));
        }
        let tally = count(&state, READ_CHANNEL, None, Some(120))
            .await
            .expect("counts");
        assert!(
            tally.at_least,
            "a walk the cap stopped must not be reported as a total"
        );
        assert!(tally.counted >= 120, "{}", tally.counted);
        assert_eq!(tally.cap, 120);
        assert!(tally.oldest_seen.is_some() && tally.newest_seen.is_some());
    }

    #[tokio::test]
    async fn a_count_since_an_instant_counts_only_what_falls_after_it() {
        let (state, fake) = testing::state();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        let base = 1_787_000_000_000_i64;
        for i in 0..10 {
            fake.seed_at(&channel, "agent", &format!("m{i}"), base + i * 1_000);
        }
        let since = crate::clock::iso_from_ms(base + 6_000);
        let tally = count(&state, READ_CHANNEL, Some(&since), None)
            .await
            .expect("counts");
        assert_eq!(
            tally.counted, 4,
            "m6 through m9, with the boundary itself counted as inside"
        );
        assert!(!tally.at_least);
    }

    // --- `#39 channel-alias` -------------------------------------------------------------------

    #[tokio::test]
    async fn the_operators_name_reaches_every_channel_this_server_hands_back() {
        // The overlay, on both of the two paths a channel leaves this module by: the LISTING, and
        // the per-channel gate every read goes through. A fix applied to only one of the two is
        // how the picker and the digest header end up disagreeing about what the channel is
        // called.
        let (state, fake, store) = testing::state_with_store();
        store
            .set_channel_alias(
                &crate::model::ChannelId(READ_CHANNEL.to_owned()),
                "the build channel",
            )
            .await
            .expect("the operator names it");

        let listed = channels(&state).await;
        let renamed = listed
            .iter()
            .find(|c| c.id.as_str() == READ_CHANNEL)
            .expect("the read channel is configured");
        assert_eq!(renamed.alias.as_deref(), Some("the build channel"));
        assert_eq!(
            renamed.label, "build noise",
            "the configured label survives"
        );
        assert_eq!(renamed.display_name(), "the build channel");

        let untouched = listed
            .iter()
            .find(|c| c.id.as_str() == WRITE_CHANNEL)
            .expect("the write channel is configured");
        assert_eq!(untouched.alias, None, "one alias must not rename the rest");
        assert_eq!(untouched.display_name(), "lead team");

        fake.seed(
            &crate::model::ChannelId(READ_CHANNEL.to_owned()),
            "agent",
            "the arm64 job failed",
        );
        let window = messages(&state, READ_CHANNEL, None).await.expect("reads");
        assert_eq!(
            window.channel.display_name(),
            "the build channel",
            "a read must come back wearing the name the operator gave it"
        );
    }

    #[tokio::test]
    async fn clearing_the_alias_puts_the_configured_label_back_everywhere() {
        let (state, _fake, store) = testing::state_with_store();
        let channel = crate::model::ChannelId(READ_CHANNEL.to_owned());
        set_channel_alias(&state, READ_CHANNEL, "the build channel")
            .await
            .expect("set");
        let cleared = clear_channel_alias(&state, READ_CHANNEL)
            .await
            .expect("clear");
        assert_eq!(cleared.alias, None);
        assert_eq!(cleared.display_name(), "build noise");
        assert!(
            store.channel_aliases().await.expect("read").is_empty(),
            "clearing must remove the row, not merely stop showing it"
        );
        assert_eq!(
            channels(&state)
                .await
                .iter()
                .find(|c| c.id == channel)
                .expect("configured")
                .display_name(),
            "build noise"
        );
    }

    #[tokio::test]
    async fn setting_an_alias_is_confined_to_the_allowlist_and_writes_nothing_when_refused() {
        // Otherwise an unconfigured snowflake grows this table one row at a time, and each row
        // names a channel this server will never show.
        let (state, _fake, store) = testing::state_with_store();
        let error = set_channel_alias(&state, "9999999999", "somewhere else")
            .await
            .expect_err("an unconfigured channel must not be nameable");
        assert!(matches!(error, OpError::UnknownChannel), "{error:?}");
        assert!(
            store.channel_aliases().await.expect("read").is_empty(),
            "a refused rename must not have written anything"
        );
        let error = clear_channel_alias(&state, "9999999999")
            .await
            .expect_err("nor clearable");
        assert!(matches!(error, OpError::UnknownChannel), "{error:?}");
    }

    #[tokio::test]
    async fn a_blank_alias_is_refused_rather_than_silently_clearing_the_name() {
        let (state, _fake, store) = testing::state_with_store();
        set_channel_alias(&state, READ_CHANNEL, "the build channel")
            .await
            .expect("set");
        let error = set_channel_alias(&state, READ_CHANNEL, "   ")
            .await
            .expect_err("a blank name must be refused");
        assert_eq!(error.code(), "bad_id", "{error}");
        assert_eq!(
            store
                .channel_aliases()
                .await
                .expect("read")
                .first()
                .map(|a| a.alias.clone()),
            Some("the build channel".to_owned()),
            "a refused rename must leave the name that was already there"
        );
    }

    #[tokio::test]
    async fn a_channel_with_no_alias_cannot_be_cleared_and_is_told_so() {
        let (state, _fake, _store) = testing::state_with_store();
        let error = clear_channel_alias(&state, READ_CHANNEL)
            .await
            .expect_err("there was nothing to clear");
        assert_eq!(error.code(), "not_found", "{error}");
    }

    #[tokio::test]
    async fn with_no_store_configured_a_channel_still_reads_under_its_configured_label() {
        // The ordinary deployment that never set `storage.path`. An alias is a DECORATION: a
        // store that refuses every call must cost the operator his aliases and nothing else —
        // certainly not the ability to read a channel at all.
        let (state, fake) =
            testing::state_with(std::sync::Arc::new(crate::store::disabled::DisabledStore));
        fake.seed(
            &crate::model::ChannelId(READ_CHANNEL.to_owned()),
            "agent",
            "the arm64 job failed",
        );
        let listed = channels(&state).await;
        assert_eq!(listed.len(), 2, "the configured channels are still listed");
        assert!(listed.iter().all(|c| c.alias.is_none()));
        let window = messages(&state, READ_CHANNEL, None)
            .await
            .expect("a channel must stay readable with no store at all");
        assert_eq!(window.channel.display_name(), "build noise");
    }

    #[tokio::test]
    async fn a_store_that_fails_degrades_to_the_configured_label_instead_of_failing_the_read() {
        let (state, _fake, store) = testing::state_with_store();
        set_channel_alias(&state, READ_CHANNEL, "the build channel")
            .await
            .expect("set");
        let named = |list: &[ChannelInfo]| {
            list.iter()
                .find(|c| c.id.as_str() == READ_CHANNEL)
                .expect("configured")
                .display_name()
                .to_owned()
        };
        // The control, so the degraded answer below is not the only answer this can give.
        assert_eq!(named(&channels(&state).await), "the build channel");

        store.fail_next("the disk is full");
        assert_eq!(
            named(&channels(&state).await),
            "build noise",
            "a backend failure must degrade to the configured label, not take the listing down"
        );
    }

    #[test]
    fn every_refusal_has_a_stable_code() {
        assert_eq!(OpError::UnknownChannel.code(), "unknown_channel");
        assert_eq!(OpError::UnknownMessage.code(), "unknown_message");
        assert_eq!(OpError::EmptyQuery.code(), "empty_query");
        assert_eq!(OpError::ChannelNotWritable.code(), "channel_not_writable");
        assert_eq!(OpError::InvalidCursor.code(), "invalid_cursor");
        assert_eq!(OpError::InvalidRange.code(), "invalid_range");
        assert_eq!(
            OpError::Discord(DiscordError::Refused("too long".to_owned())).code(),
            "refused"
        );
        assert_eq!(
            OpError::Discord(DiscordError::Transport("down".to_owned())).code(),
            "discord_error"
        );
    }
}
