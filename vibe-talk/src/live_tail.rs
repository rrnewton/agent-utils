//! Messages a provider's own event stream delivered with their text, published as they arrive.
//! `#452 slack-live-ingest`.
//!
//! A content-free hint ([`crate::live::LiveHub::queue_hint`]) throws away the text the stream
//! already carried and reads the channel again through the provider: a round trip per arrival,
//! spaced by the hint floor under load. A stream record that carries the message is published
//! instead, straight into the live hub and from there into the message cache, after the same
//! normalization a read does ([`crate::chat::ChatClient::pushed_message`]). The poller keeps
//! running beside it: a read of a message already pushed publishes nothing new
//! ([`crate::live::CREATED_MEMORY`]), and the poller and the relist ([`crate::relist`]) find what
//! the stream never carries — other bots' posts, edits and deletions.
//!
//! Only the configured channels of a provider that takes pushed messages are published to: a
//! record for any other conversation is acknowledged and dropped, so that a channel taken off the
//! list while an adapter still tails it costs nothing. A record that says the stream lost records
//! (`GAP`) reads every such channel again at once, and relists it; a reaction record wakes the
//! poller, whose read reports the reaction set as it stands (`#378 live-reactions`).
//!
//! A chat bridge's own records (`agentctl.chat-event.v1`, `#708 vt-live-push`) arrive the same way:
//! its posts are published as a stream record's message is, and a reaction it added is put on the
//! message at once, as present (`add_reaction`). The owner's stream never carries either.

use crate::chat::{ChatClient as _, ChatError};
use crate::live::Published;
use crate::model::{ChannelId, ChannelInfo};
use crate::slack::ids::ChannelRef;
use crate::slack::tail::TailRecord;
use crate::state::AppState;

/// Why a channel whose event stream came back after a silence is not presented as complete.
/// `#708 vt-live-push`.
const STREAM_AWAY: &str =
    "The chat service's event stream was away; what it missed has not been read yet.";
/// Why a channel whose event stream lost records is not presented as complete.
const STREAM_LOST: &str =
    "The chat service's event stream lost messages; they have not been read yet.";
/// Why a channel a record of which could not be applied is not presented as complete.
const STREAM_UNAPPLIED: &str =
    "A message the chat service's event stream announced has not been read yet.";

/// Put `emoji` on message `id` of `channel` as present, from the copy readers were last given
/// or, failing that, the message cache's: `Some(true)` when it was not there and the message went
/// out as an update, `Some(false)` when it was already there, `None` when no copy can say. `#708
/// vt-live-push`.
///
/// "Present", not "one more": a bridge's record says it added the reaction, and the set a read
/// reported may already count it, so a tally is never raised for an emoji already on the message.
/// A count short by a second reactor of the same emoji is put right by the next read, which
/// reports the set as it stands. A copy that says nothing of reactions is taken as having none
/// only while it is new enough to have had no time to collect any unseen ([`PUSH_STALE_MS`]).
async fn add_reaction(
    state: &AppState,
    channel: &ChannelId,
    id: &crate::model::MessageId,
    emoji: &str,
    custom: bool,
    now_ms: i64,
) -> Option<bool> {
    // The newer of the two: the copy readers were last given may be older than one a read has
    // since put in the cache, and republished whole it would put the older text back.
    let known = state.live.known_copy(channel, id);
    let held = state.message_cache.held_message(channel, id).await;
    let mut message = match (known, held) {
        (Some(known), Some(held)) => {
            if held.observed_ms.unwrap_or(i64::MIN) > known.observed_ms.unwrap_or(i64::MIN) {
                held
            } else {
                known
            }
        }
        (known, held) => known.or(held)?,
    };
    let reactions = match message.reactions.as_mut() {
        Some(reactions) => reactions,
        None => {
            let fresh = id
                .created_at_ms()
                .is_some_and(|at| now_ms.saturating_sub(at) <= crate::live::PUSH_STALE_MS);
            if !fresh {
                return None;
            }
            message.reactions.insert(Vec::new())
        }
    };
    let bare = |text: &str| text.replace('\u{FE0F}', "");
    if reactions
        .iter()
        .any(|held| held.custom == custom && bare(&held.emoji) == bare(emoji))
    {
        return Some(false);
    }
    reactions.push(crate::model::Reaction {
        emoji: emoji.to_owned(),
        custom,
        custom_id: None,
        count: 1,
        owner_reacted: false,
    });
    state.live.publish_update(channel, message);
    Some(true)
}

/// What one record did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TailOutcome {
    /// A new message went out to at least one channel.
    Published,
    /// Every channel it belongs to already had the message.
    Known,
    /// The message is history a page has already read ([`Published::History`]).
    History,
    /// A record with nothing to publish: a heartbeat, a kind this server does not act on, or a
    /// message a read would also leave out (a system event).
    Ignored,
    /// The conversation is not one of this server's channels for a provider that takes pushes.
    Unlisted,
    /// The stream lost records: this many channels are read again and relisted now.
    Gap(usize),
    /// A reaction: this many channels' pollers were woken.
    Hinted(usize),
}

/// The configured channels of the provider that takes pushed messages for `conversation`, each
/// with the thread it is narrowed to, if any.
fn channels_of(state: &AppState, conversation: Option<&str>) -> Vec<(ChannelInfo, ChannelRef)> {
    state
        .all_channels()
        .into_iter()
        .filter_map(|channel| {
            let target = ChannelRef::parse(channel.id.as_str())?;
            if conversation.is_some_and(|wanted| wanted != target.conversation) {
                return None;
            }
            state
                .providers
                .client_for(&channel.id)
                .is_some_and(|client| client.takes_pushed_messages(&channel.id))
                .then_some((channel, target))
        })
        .collect()
}

/// The conversations this server publishes stream records for: what an adapter should tail.
#[must_use]
pub fn conversations(state: &AppState) -> Vec<String> {
    let mut conversations: Vec<String> = channels_of(state, None)
        .into_iter()
        .map(|(_, target)| target.conversation)
        .collect();
    conversations.sort_unstable();
    conversations.dedup();
    conversations
}

/// Whether `channel`'s provider is polled, and its key.
fn polled(state: &AppState, channel: &ChannelId) -> Option<String> {
    let key = state.providers.key_for(channel)?;
    state
        .providers
        .entry(&key)
        .is_some_and(|entry| entry.live_poll_seconds > 0)
        .then_some(key)
}

/// Mark `channel` as possibly missing what its stream lost or could not apply, for `why`, when a
/// poller runs to clear the mark: with none, nothing would ever read the channel again to prove it
/// whole, and every answer would say so for ever. `#708 vt-live-push`.
fn mark(state: &AppState, channel: &ChannelId, why: &'static str) {
    if polled(state, channel).is_some() {
        state
            .live
            .note_stream_unreconciled(channel, state.live.reconciled_ms(channel), why);
    }
}

/// Wake the poller of `channel`'s provider, when it polls. Whether a poller was woken.
fn wake(state: &AppState, channel: &ChannelId) -> bool {
    let Some(key) = polled(state, channel) else {
        return false;
    };
    let queue_key = state.providers.is_multi().then_some(key.as_str());
    // Queued or already pending: either way the poller reads it.
    let _ = state.live.queue_hint(queue_key, channel);
    true
}

/// Act on one checked record, at `now_ms`.
///
/// # Errors
///
/// [`ChatError`] from the provider's normalization of a message: [`ChatError::Shape`] for an
/// object it does not recognize as one of its messages.
pub async fn ingest(
    state: &AppState,
    record: TailRecord,
    now_ms: i64,
) -> Result<TailOutcome, ChatError> {
    // `#595 cache-full-history`. A stream that comes back after a silence may have missed
    // anything while it was away: its channels' hot walks read what it did not carry.
    // `#708 vt-live-push`: a bridge's own record says nothing of whether the stream is alive, and
    // counted as one it would hide the stream's silence.
    if !record.from_bridge() && state.live.note_stream_record(now_ms) {
        let channels = channels_of(state, None);
        for (channel, _) in &channels {
            state.live.request_rewalk(&channel.id);
            // `#708 vt-live-push`: until it is read again, the channel may lack what the stream
            // did not carry while it was away.
            mark(state, &channel.id, STREAM_AWAY);
        }
        tracing::info!(
            channels = channels.len(),
            "the provider's event stream is back after a silence; re-reading its channels' recent \
             history"
        );
    }
    match record {
        TailRecord::Control => Ok(TailOutcome::Ignored),
        TailRecord::Other { sequence, why } => {
            tracing::debug!(sequence, why, "a stream record with nothing to publish");
            Ok(TailOutcome::Ignored)
        }
        TailRecord::Gap { .. } => {
            // A gap names sequence numbers, never a conversation: every channel the stream
            // feeds may have lost a message, so each is read forward from its cursor now, and
            // relisted for what a forward read cannot see.
            let channels = channels_of(state, None);
            for (channel, _) in &channels {
                wake(state, &channel.id);
                state.live.request_relist(&channel.id);
                state.live.request_rewalk(&channel.id);
                mark(state, &channel.id, STREAM_LOST);
            }
            tracing::warn!(
                channels = channels.len(),
                "the provider's event stream lost records; reading its channels again"
            );
            Ok(TailOutcome::Gap(channels.len()))
        }
        TailRecord::Unapplied { conversation, why } => {
            tracing::debug!(why, "a stream record left to the poller");
            let channels = channels_of(state, Some(&conversation));
            if channels.is_empty() {
                return Ok(TailOutcome::Unlisted);
            }
            let woken = channels
                .iter()
                .filter(|(channel, _)| wake(state, &channel.id))
                .count();
            Ok(TailOutcome::Hinted(woken))
        }
        TailRecord::ReactionAdded {
            conversation,
            ts,
            emoji,
            custom,
        } => {
            let channels = channels_of(state, Some(&conversation));
            if channels.is_empty() {
                return Ok(TailOutcome::Unlisted);
            }
            let Some(id) = crate::slack::ids::message_id_from_ts(&ts) else {
                return Ok(TailOutcome::Ignored);
            };
            let mut outcome = TailOutcome::Ignored;
            let mut woken = 0;
            for (channel, _) in &channels {
                match add_reaction(state, &channel.id, &id, &emoji, custom, now_ms).await {
                    Some(true) => outcome = TailOutcome::Published,
                    Some(false) => {
                        if outcome == TailOutcome::Ignored {
                            outcome = TailOutcome::Known;
                        }
                    }
                    // Not a message this server holds a copy of that can say: the poller's read
                    // brings the set as it stands.
                    None => woken += usize::from(wake(state, &channel.id)),
                }
            }
            if outcome == TailOutcome::Ignored && woken > 0 {
                outcome = TailOutcome::Hinted(woken);
            }
            Ok(outcome)
        }
        TailRecord::Reaction { conversation, .. } => {
            let channels = channels_of(state, Some(&conversation));
            if channels.is_empty() {
                return Ok(TailOutcome::Unlisted);
            }
            let woken = channels
                .iter()
                .filter(|(channel, _)| wake(state, &channel.id))
                .count();
            Ok(TailOutcome::Hinted(woken))
        }
        TailRecord::Message {
            conversation,
            ts,
            thread_ts,
            broadcast,
            raw,
            ..
        } => {
            let thread = thread_ts.clone().unwrap_or_else(|| ts.clone());
            let targets: Vec<(ChannelInfo, bool)> = channels_of(state, Some(&conversation))
                .into_iter()
                .filter_map(|(channel, target)| match target.thread_ts {
                    // The whole conversation: a reply read apart from its timeline, unless it
                    // was also sent to the conversation.
                    None => Some((channel, thread_ts.is_some() && !broadcast)),
                    // A channel narrowed to one thread holds that thread's root and replies as
                    // its own messages.
                    Some(narrowed) => (narrowed == thread).then_some((channel, false)),
                })
                .collect();
            if targets.is_empty() {
                return Ok(TailOutcome::Unlisted);
            }
            let mut outcome = TailOutcome::Ignored;
            let id = crate::slack::ids::message_id_from_ts(&ts);
            for (channel, reply) in targets {
                // History is told by the id alone, before the provider normalizes anything: a
                // stream replaying what a page has already read costs no author lookup.
                if id
                    .as_ref()
                    .is_none_or(|id| state.live.is_history(&channel.id, id, reply, now_ms))
                {
                    if outcome == TailOutcome::Ignored {
                        outcome = TailOutcome::History;
                    }
                    continue;
                }
                let pushed = state
                    .providers
                    .pushed_message(&channel.id, raw.clone())
                    .await;
                if let Err(error) = &pushed {
                    if !matches!(error, ChatError::Shape(_)) {
                        // `#708 vt-live-push`: seen, and not applied. The poller reads it; until
                        // then the channel is not presented as complete.
                        wake(state, &channel.id);
                        mark(state, &channel.id, STREAM_UNAPPLIED);
                    }
                }
                let Some(mut message) = pushed? else {
                    continue;
                };
                crate::ops::prepare_arrival(
                    state,
                    crate::auth::Scope::Write,
                    &channel.id,
                    &mut message,
                )
                .await;
                let published = state
                    .live
                    .publish_pushed(&channel.id, message, reply, now_ms);
                outcome = match (outcome, published) {
                    (TailOutcome::Published, _) | (_, Published::New | Published::Updated) => {
                        TailOutcome::Published
                    }
                    (TailOutcome::Known, _) | (_, Published::Known) => TailOutcome::Known,
                    (_, Published::History) => TailOutcome::History,
                };
            }
            Ok(outcome)
        }
    }
}
