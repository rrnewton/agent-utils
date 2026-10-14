//! A reply marks read what it answers, here and at the provider. `#292 reply-marks-read`.
//!
//! # The rule
//!
//! The owner, 2026-10-14: pressing Reply on a message and opening its thread to write in it both make
//! a reply to the message before, and they marked read differently. The rule agreed:
//!
//! 1. The first reply that starts a new thread marks the thread's root read, whichever way it was
//!    started — Reply on the root, or the thread opened and written in while it had no replies.
//! 2. Later messages written in an existing thread mark nothing read on their own.
//! 3. A targeted reply — one sent with `reply_to`, which the Reply and Quote reply buttons set —
//!    marks the message it answers read, on the main channel or inside a thread.
//! 4. Marking read is what the page's own read action does: Done, held on this server, and the
//!    provider's read marker moved through the message where the provider lets this server move it,
//!    the marker covers no more than the channel, and the move is forward. Never backward, and never
//!    the rest of a space the channel is one thread of.
//! 5. A voice agent's send follows the button it stands for. `post_reply` with `reply_to` is a
//!    targeted reply; it has no thread argument, so it never stands for the thread's composer. It
//!    marks only once the post is confirmed.
//!
//! The page decides 1 to 3 for what it sends, because only the page knows which way a reply was
//! started, and marks Done itself, by the Done button's own act, so the Undo chip and the bookmark
//! follow. What it asks of this server is the provider's half, through
//! `POST /api/v1/channels/{id}/upstream-read` with `after_reply: true` ([`mark_upstream`]). A voice
//! agent's send has no page in the loop — with "Ask me before the voice agent sends" off it is posted
//! by the tool call itself — so this server marks both halves for it ([`after_voice_send`]).
//!
//! # Never backward, never twice
//!
//! The provider's marker is one cursor per conversation, so marking a message moves it through that
//! message and everything before it. A reply to an older message must not move it back behind a
//! newer one already marked, and a second reply to the same message must not mark it again. The
//! bridge that implements the mark promises the first (it reads its cursor and writes nothing when
//! the cursor is at or past the message), but asking it costs a read of that cursor, and Google Chat
//! allows about two of those per user per minute. So this server keeps, per channel, the newest
//! message it has marked through — by a reply here or by the owner's own **Mark read in** — and
//! does not ask again for that message or any message before it. Message ids are snowflakes, which
//! order by creation time ([`MessageId::numeric`]); an id that is not a number cannot be ordered, so
//! only that exact id is remembered, and the bridge's own check is the floor for it.
//!
//! The floor lives in memory. After a restart the first reply-mark of a channel asks the provider,
//! whose own check keeps it forward. A mark that fails moves no floor, so the next reply asks again.
//!
//! # Never wider than the channel
//!
//! The marker a reply's mark moves is the one **Mark read in** moves, and that covers the whole space.
//! When the owner presses that item it says so and the choice is his. A reply's mark is made with
//! nobody pressing anything — for a voice agent's reply, with nobody watching — so it is made only
//! where the marker covers no more than vibe-talk holds of the channel, the line the automatic clear
//! of `#232 upstream-mark-read` draws too (its bridge refuses such a channel with `thread_channel`).
//! On a registration narrowed to one thread of a larger space, marking a reply's message would mark
//! every earlier message of the whole space read in the provider's own app, messages vibe-talk never
//! received, and nothing here could move the marker back.
//!
//! So before the first reply-mark of a channel the provider is asked what its marker covers there
//! ([`crate::chat::ChatClient::upstream_read_scope`]): a bridge answers from its own timeline, which
//! names the one conversation a narrowed registration is scoped to. Only
//! [`UpstreamReadScope::Channel`] is marked. A narrowed channel ([`UpstreamMark::ThreadChannel`]) and
//! one whose provider cannot say ([`UpstreamMark::ScopeUnknown`]) ask the marker nothing; the Done
//! stands. The answer is kept beside the channel's floor, in memory, and so is a `thread_channel`
//! refusal from the provider's own mark, whoever asked it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, PoisonError};

use crate::chat::{ChatError, UpstreamReadScope};
use crate::model::{ChannelId, ChannelInfo, MessageId};
use crate::read_reason::ReadReason;
use crate::state::AppState;
use crate::store::StoreError;

/// How many ids that cannot be ordered are remembered per channel, newest kept.
const UNORDERED_MEMORY: usize = 64;

/// What one reply-mark did at the provider.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamMark {
    /// The provider was asked to move its marker through the message, and took it.
    Marked,
    /// This message was marked already; nothing was asked.
    AlreadyMarked,
    /// A later message was marked already, so the marker is past this one; nothing was asked.
    Behind,
    /// The channel's provider does not let this server move its marker.
    Unsupported,
    /// The channel is one thread of a larger space, whose marker would also clear messages this
    /// server never received; nothing was asked.
    ThreadChannel,
    /// The channel's provider cannot say whether its marker covers more than the channel; nothing
    /// was asked.
    ScopeUnknown,
}

/// Every channel's floor, each behind a lock of its own, so a slow provider in one channel holds up
/// no mark in another.
#[derive(Debug, Default)]
pub struct ReplyReads {
    floors: std::sync::Mutex<HashMap<ChannelId, Arc<tokio::sync::Mutex<Floor>>>>,
}

/// One channel's floor: the newest message marked through, the unorderable ids marked, and what
/// the provider's marker covers there, once the provider has said
/// ([`UpstreamReadScope::Channel`] or [`UpstreamReadScope::Wider`]; an unknown answer is asked again).
#[derive(Debug, Default)]
struct Floor {
    through: Option<(u64, MessageId)>,
    unordered: VecDeque<MessageId>,
    scope: Option<UpstreamReadScope>,
}

impl Floor {
    /// Whether marking `message` would ask for nothing new.
    fn covers(&self, message: &MessageId) -> Option<UpstreamMark> {
        match message.numeric() {
            Some(position) => match &self.through {
                Some((floor, id)) if id == message => {
                    debug_assert_eq!(*floor, position);
                    Some(UpstreamMark::AlreadyMarked)
                }
                Some((floor, _)) if position <= *floor => Some(UpstreamMark::Behind),
                _ => None,
            },
            None => self
                .unordered
                .contains(message)
                .then_some(UpstreamMark::AlreadyMarked),
        }
    }

    /// Note that the provider's marker went through `message`.
    fn raise(&mut self, message: &MessageId) {
        match message.numeric() {
            Some(position) => {
                if self
                    .through
                    .as_ref()
                    .is_none_or(|(floor, _)| position > *floor)
                {
                    self.through = Some((position, message.clone()));
                }
            }
            None => {
                if !self.unordered.contains(message) {
                    self.unordered.push_back(message.clone());
                    while self.unordered.len() > UNORDERED_MEMORY {
                        self.unordered.pop_front();
                    }
                }
            }
        }
    }
}

impl ReplyReads {
    /// No floors yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `channel`'s floor, made empty the first time it is asked for. Bounded by the channels the
    /// server answers for, which are the only ones a mark can name.
    fn floor(&self, channel: &ChannelId) -> Arc<tokio::sync::Mutex<Floor>> {
        Arc::clone(
            self.floors
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(channel.clone())
                .or_default(),
        )
    }

    /// Note that the provider's marker for `channel` went through `message` by some other act — the
    /// owner's own **Mark read in** — so a later reply to it, or to anything before it, asks nothing.
    pub async fn note_marked(&self, channel: &ChannelId, message: &MessageId) {
        self.floor(channel).lock().await.raise(message);
    }

    /// Note why the provider refused a mark in `channel`, whoever asked it: a `thread_channel`
    /// refusal says the marker covers more than the channel, so no reply asks it again.
    pub async fn note_refused(&self, channel: &ChannelId, error: &ChatError) {
        if is_thread_channel(error) {
            self.floor(channel).lock().await.scope = Some(UpstreamReadScope::Wider);
        }
    }
}

/// Whether `error` is the provider refusing a channel whose marker covers more than it: the bridge
/// contract's `409` with `"code":"thread_channel"`.
fn is_thread_channel(error: &ChatError) -> bool {
    matches!(
        error.cause(),
        ChatError::Status { status: 409, body } if body.contains("thread_channel")
    )
}

/// Whether `channel`'s own provider lets this server move its read marker.
#[must_use]
pub fn supported(state: &AppState, channel: &ChannelId) -> bool {
    state
        .providers
        .client_for(channel)
        .is_some_and(|client| client.supports_upstream_read_mark())
}

/// Move the provider's read marker for `channel` through `message`, because a reply answered it —
/// unless this server has marked that message, or a later one, already, or the marker covers more
/// than the channel, or the provider cannot say whether it does. See the module note.
///
/// One at a time per channel: the channel's floor is read and raised around the provider's answer,
/// so two replies to the same message landing together ask once, and the marker's scope is asked
/// once.
///
/// # Errors
///
/// The provider's own failure, to say what its marker covers or to move it, which raises no floor.
pub async fn mark_upstream(
    state: &AppState,
    channel: &ChannelId,
    message: &MessageId,
) -> Result<UpstreamMark, ChatError> {
    if !supported(state, channel) {
        return Ok(UpstreamMark::Unsupported);
    }
    let floor = state.reply_reads.floor(channel);
    let mut floor = floor.lock().await;
    if let Some(covered) = floor.covers(message) {
        return Ok(covered);
    }
    let scope = match floor.scope {
        Some(known) => known,
        None => {
            let told = ReadReason::ReplyMark
                .scope(state.chat.upstream_read_scope(channel))
                .await?;
            if told != UpstreamReadScope::Unknown {
                floor.scope = Some(told);
            }
            told
        }
    };
    match scope {
        UpstreamReadScope::Channel => {}
        UpstreamReadScope::Wider => return Ok(UpstreamMark::ThreadChannel),
        UpstreamReadScope::Unknown => return Ok(UpstreamMark::ScopeUnknown),
    }
    match state.chat.mark_read_upstream(channel, message).await {
        Ok(()) => {
            floor.raise(message);
            Ok(UpstreamMark::Marked)
        }
        // The provider knows better than its earlier word: kept, and nothing is asked again.
        Err(error) if is_thread_channel(&error) => {
            floor.scope = Some(UpstreamReadScope::Wider);
            Ok(UpstreamMark::ThreadChannel)
        }
        Err(error) => Err(error),
    }
}

/// What a voice agent's confirmed reply marked.
#[derive(Debug)]
pub struct VoiceMark {
    /// Whether the message answered was newly Done on this server; false when it was Done already,
    /// or when no store is configured to hold it.
    pub done: bool,
    /// The provider's half, which runs after the answer: await it to know what it did.
    pub upstream: Option<tokio::task::JoinHandle<Result<UpstreamMark, ChatError>>>,
}

/// After a voice agent's reply to `reply_to` in `channel` was posted: mark what it answered read,
/// as the Reply button's send does. **Only after a confirmed post** — a direct send that answered
/// `SENT`, or a confirmed proposal that posted whole; a partial post or a failure marks nothing.
///
/// Done is written here, before the tool's answer, because it is one write to the store. The
/// provider's half is a request to the chat service, sometimes a slow one, so it runs on its own and
/// the answer does not wait for it; its failure is logged. It is [`mark_upstream`], so a channel
/// whose marker covers more than the channel, or whose provider cannot say, is not marked there:
/// with "Ask me before the voice agent sends" off nobody is watching to object.
pub async fn after_voice_send(
    state: &AppState,
    channel: &ChannelInfo,
    reply_to: &str,
) -> VoiceMark {
    let message = MessageId(reply_to.to_owned());
    let done = match state
        .store
        .dismiss(&channel.id, std::slice::from_ref(&message))
        .await
    {
        Ok(count) => count > 0,
        // Nothing to hold it in: the deployment has no Done at all, so there is nothing to mark.
        Err(StoreError::Unavailable(_)) => false,
        Err(error) => {
            tracing::warn!(
                channel = %channel.id,
                %error,
                "a voice reply was sent, and the message it answers could not be marked Done"
            );
            false
        }
    };
    let upstream = supported(state, &channel.id).then(|| {
        let state = state.clone();
        let channel = channel.id.clone();
        tokio::spawn(async move {
            let marked = mark_upstream(&state, &channel, &message).await;
            if let Err(error) = &marked {
                tracing::warn!(
                    channel = %channel,
                    %error,
                    "a voice reply was sent, and the chat service did not mark the message it \
                     answers read"
                );
            }
            marked
        })
    });
    VoiceMark { done, upstream }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> MessageId {
        MessageId(text.to_owned())
    }

    #[test]
    fn a_floor_covers_its_own_message_and_everything_before_it_and_nothing_after() {
        let mut floor = Floor::default();
        assert_eq!(floor.covers(&id("1000000000000000005")), None);
        floor.raise(&id("1000000000000000005"));
        assert_eq!(
            floor.covers(&id("1000000000000000005")),
            Some(UpstreamMark::AlreadyMarked)
        );
        assert_eq!(
            floor.covers(&id("1000000000000000004")),
            Some(UpstreamMark::Behind)
        );
        // Shorter is older: snowflakes order as numbers, not as text.
        assert_eq!(
            floor.covers(&id("999999999999999999")),
            Some(UpstreamMark::Behind)
        );
        assert_eq!(floor.covers(&id("1000000000000000006")), None);
        // A floor never comes down.
        floor.raise(&id("1000000000000000003"));
        assert_eq!(
            floor.covers(&id("1000000000000000004")),
            Some(UpstreamMark::Behind)
        );
    }

    #[test]
    fn an_id_that_cannot_be_ordered_covers_only_itself_and_is_forgotten_oldest_first() {
        let mut floor = Floor::default();
        floor.raise(&id("1000000000000000005"));
        assert_eq!(floor.covers(&id("spaces/a/messages/b")), None);
        floor.raise(&id("spaces/a/messages/b"));
        assert_eq!(
            floor.covers(&id("spaces/a/messages/b")),
            Some(UpstreamMark::AlreadyMarked)
        );
        assert_eq!(floor.covers(&id("spaces/a/messages/c")), None);
        for n in 0..UNORDERED_MEMORY {
            floor.raise(&id(&format!("m{n}")));
        }
        assert_eq!(floor.covers(&id("spaces/a/messages/b")), None);
        assert_eq!(floor.covers(&id("m1")), Some(UpstreamMark::AlreadyMarked));
    }
}
