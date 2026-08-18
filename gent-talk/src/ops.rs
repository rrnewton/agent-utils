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
) -> Result<(ChannelInfo, Vec<Message>), OpError> {
    let info = allowed(state, channel_id)?;
    let limit = state.effective_limit(limit);
    let messages = state.discord.fetch_recent(&info.id, limit).await?;
    Ok((info, messages))
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
    let (info, messages) = messages(state, channel_id, limit).await?;
    let wanted = MessageId(message_id.to_owned());
    let message = messages
        .into_iter()
        .find(|m| m.id == wanted)
        .ok_or(OpError::UnknownMessage)?;
    Ok((info, message))
}

/// One speakable line per recent message. Read scope.
///
/// # Errors
///
/// As [`messages`].
pub async fn digest(
    state: &AppState,
    channel_id: &str,
    limit: Option<u16>,
    width: Option<u16>,
) -> Result<(ChannelInfo, Vec<DigestEntry>), OpError> {
    let (info, messages) = messages(state, channel_id, limit).await?;
    let width = usize::from(width.unwrap_or(0));
    let width = if width == 0 {
        DEFAULT_SUMMARY_CHARS
    } else {
        width
    };
    Ok((info, summary::digest(&messages, width)))
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
