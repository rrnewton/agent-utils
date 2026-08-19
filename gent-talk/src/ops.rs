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
use crate::model::{ChannelInfo, Message, MessageId};
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
    #[error(transparent)]
    Discord(#[from] DiscordError),
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
            Self::Discord(DiscordError::Refused(_)) => "refused",
            Self::Discord(_) => "discord_error",
        }
    }
}

/// Resolve a caller-supplied channel id against the allowlist.
///
/// This is the single gate every channel-scoped operation goes through. A snowflake that is not
/// configured does not exist here, however many channels the bot is actually in.
fn allowed(state: &AppState, channel_id: &str) -> Result<ChannelInfo, OpError> {
    state
        .channel(channel_id)
        .cloned()
        .ok_or(OpError::UnknownChannel)
}

/// The channels this server is configured for. Read scope.
#[must_use]
pub fn channels(state: &AppState) -> Vec<ChannelInfo> {
    state.config.channels.clone()
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
    let channel = allowed(state, channel_id)?;
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
    let channel = allowed(state, channel_id)?;
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
    let channel = allowed(state, channel_id)?;
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
    let info = allowed(state, channel_id)?;
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
) -> Result<(ChannelInfo, Message), OpError> {
    let info = allowed(state, channel_id)?;
    if !info.writable {
        return Err(OpError::ChannelNotWritable);
    }
    let reply_to = reply_to.map(|id| MessageId(id.to_owned()));
    let mut posted = state
        .discord
        .post_message(&info.id, text, reply_to.as_ref())
        .await?;
    stamp(state, std::slice::from_mut(&mut posted));
    Ok((info, posted))
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let (info, posted) = reply(&state, WRITE_CHANNEL, "shipped it", None)
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
