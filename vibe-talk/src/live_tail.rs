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

use crate::chat::{ChatClient as _, ChatError};
use crate::live::Published;
use crate::model::{ChannelId, ChannelInfo};
use crate::slack::ids::ChannelRef;
use crate::slack::tail::TailRecord;
use crate::state::AppState;

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

/// Wake the poller of `channel`'s provider, when it polls. Whether a poller was woken.
fn wake(state: &AppState, channel: &ChannelId) -> bool {
    let Some(key) = state.providers.key_for(channel) else {
        return false;
    };
    let polls = state
        .providers
        .entry(&key)
        .is_some_and(|entry| entry.live_poll_seconds > 0);
    if !polls {
        return false;
    }
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
    match record {
        TailRecord::Control | TailRecord::Other { .. } => Ok(TailOutcome::Ignored),
        TailRecord::Gap { .. } => {
            // A gap names sequence numbers, never a conversation: every channel the stream
            // feeds may have lost a message, so each is read forward from its cursor now, and
            // relisted for what a forward read cannot see.
            let channels = channels_of(state, None);
            for (channel, _) in &channels {
                wake(state, &channel.id);
                state.live.request_relist(&channel.id);
            }
            tracing::warn!(
                channels = channels.len(),
                "the provider's event stream lost records; reading its channels again"
            );
            Ok(TailOutcome::Gap(channels.len()))
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
                    .is_none_or(|id| state.live.is_history(&channel.id, id, now_ms))
                {
                    if outcome == TailOutcome::Ignored {
                        outcome = TailOutcome::History;
                    }
                    continue;
                }
                let Some(mut message) = state
                    .providers
                    .pushed_message(&channel.id, raw.clone())
                    .await?
                else {
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
