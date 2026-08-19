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
    let messages = state.discord.fetch_recent(&channel.id, limit).await?;
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
    let messages = state.discord.fetch_recent(&info.id, limit).await?;
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
    let posted = state
        .discord
        .post_message(&info.id, text, reply_to.as_ref())
        .await?;
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

    #[test]
    fn every_refusal_has_a_stable_code() {
        assert_eq!(OpError::UnknownChannel.code(), "unknown_channel");
        assert_eq!(OpError::UnknownMessage.code(), "unknown_message");
        assert_eq!(OpError::EmptyQuery.code(), "empty_query");
        assert_eq!(OpError::ChannelNotWritable.code(), "channel_not_writable");
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
