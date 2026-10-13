//! A channel's most recent messages as one read, and the shadow that checks a bridge's Flat view
//! against `/messages` before Flat is trusted to answer it. `#208 persistent-cache`, milestone
//! `bridge-recent-flat`.
//!
//! # Why
//!
//! A compatible bridge answers `/messages` by running a provider command — about two seconds,
//! through one command slot — and answers the newest page of its Flat view from a copy of the
//! channel it keeps current, in milliseconds. Both list the channel's messages, replies included,
//! newest last, under the same ids. `recent_view = "flat"` reads the second instead of the first
//! (see [`crate::discord::http`]); this module is what makes that switch something to decide on
//! evidence rather than on faith.
//!
//! # The shadow
//!
//! With `recent_view_shadow = true` (and `recent_view = "messages"`), every recent read through
//! [`read`] is followed by a read of the newest Flat page of the same size, in a detached task so
//! the caller never waits for it, at most one per channel at a time. The two answers are compared
//! over the stretch both cover — from the newer of their oldest messages (or the channel's start,
//! for a read that came back short) up to the newest message `/messages` returned — and two things
//! are counted per channel:
//!
//! * **id mismatches**: messages inside that stretch that only one of the two listed;
//! * **content-hash mismatches**: messages both listed whose text differs, compared by the same
//!   [`crate::summarize::content_hash`] the summary cache keys on.
//!
//! A Flat message newer than everything `/messages` returned is not a mismatch: it arrived after
//! that read. Counts only, in memory, in the diagnostics report. One `info` line per comparison
//! that found a difference carries the counts, and a `debug` line the message ids — never text.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::chat::{ChatError, RecentRead};
use crate::config::{ProviderKind, RecentView};
use crate::model::{ChannelId, Message, MessageId};
use crate::state::AppState;
use crate::threads::{TimelinePage, TimelineRequest, TimelineView};

/// Read `channel`'s most recent `limit` messages through its provider, and shadow the read when
/// that provider asks for it.
///
/// # Errors
///
/// As [`crate::chat::ChatClient::fetch_recent_dated`]. The shadow never fails a read.
pub async fn read(
    state: &AppState,
    channel: &ChannelId,
    limit: u16,
) -> Result<RecentRead, ChatError> {
    let read = state.chat.fetch_recent_dated(channel, limit).await?;
    if shadowed(state, channel) {
        shadow(state, channel, limit, &read.messages);
    }
    Ok(read)
}

/// Whether recent reads of `channel` are `/messages` reads its provider wants checked against Flat.
#[must_use]
pub fn shadowed(state: &AppState, channel: &ChannelId) -> bool {
    state
        .providers
        .key_for(channel)
        .and_then(|key| state.config.provider(&key))
        .is_some_and(|provider| match &provider.kind {
            ProviderKind::Discord(discord) => {
                discord.recent.shadow && discord.recent.view == RecentView::Messages
            }
            ProviderKind::Slack(_) => false,
        })
}

/// Start the Flat read that checks one `/messages` read, unless one for `channel` is running.
fn shadow(state: &AppState, channel: &ChannelId, limit: u16, messages: &[Message]) {
    let Some(running) = Running::claim(&state.recent_view_shadow, channel) else {
        return;
    };
    let state = state.clone();
    let messages = messages.to_vec();
    tokio::spawn(async move {
        let request = TimelineRequest {
            view: TimelineView::Flat,
            limit,
            ..TimelineRequest::default()
        };
        let channel = running.channel.clone();
        let flat = state.chat.fetch_timeline(&channel, &request).await;
        // Released before counting, so a reader of the counts can start the next one at once.
        drop(running);
        state
            .recent_view_shadow
            .finish(&channel, &messages, limit, flat);
    });
}

/// What one channel's shadow has counted since this process started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    /// Flat reads started to check a `/messages` read.
    pub reads: u64,
    /// `/messages` reads left unchecked because a Flat read of the channel was still running.
    pub skipped: u64,
    /// Flat reads that failed, so nothing was compared.
    pub failed: u64,
    /// Flat reads that answered and were compared.
    pub compared: u64,
    /// Of those, how many carried `as_of`: the bridge served a copy it could not bring up to date,
    /// and a mismatch beside one is explained by that.
    pub stale: u64,
    /// Comparisons that found at least one mismatch of either kind.
    pub mismatched: u64,
    /// Messages, summed over every comparison, inside the stretch both reads cover that only one
    /// of them listed.
    pub id_mismatches: u64,
    /// Messages, summed over every comparison, both reads listed with different content hashes.
    pub hash_mismatches: u64,
}

/// One channel's [`Tally`], as the diagnostics report carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChannelTally {
    /// The channel that was read.
    pub channel_id: ChannelId,
    /// What its shadow counted.
    #[serde(flatten)]
    pub tally: Tally,
}

/// Every channel's shadow counts, and which channels have a Flat read running.
#[derive(Debug, Default)]
pub struct Shadow {
    tallies: Mutex<BTreeMap<ChannelId, Tally>>,
    running: Mutex<BTreeSet<ChannelId>>,
}

impl Shadow {
    /// Nothing counted yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What `channel`'s shadow has counted, zero when it has counted nothing.
    #[must_use]
    pub fn tally(&self, channel: &ChannelId) -> Tally {
        lock(&self.tallies)
            .get(channel)
            .copied()
            .unwrap_or_default()
    }

    fn count(&self, channel: &ChannelId, change: impl FnOnce(&mut Tally)) {
        change(lock(&self.tallies).entry(channel.clone()).or_default());
    }

    /// Fold one finished Flat read into `channel`'s counts.
    fn finish(
        &self,
        channel: &ChannelId,
        messages: &[Message],
        limit: u16,
        flat: Result<TimelinePage, ChatError>,
    ) {
        let page = match flat {
            Ok(page) => page.into_recent(limit),
            Err(error) => {
                tracing::warn!(channel = %channel, %error, "recent view shadow: the Flat read failed");
                self.count(channel, |tally| tally.failed += 1);
                return;
            }
        };
        let difference = compare(messages, &page.messages, limit);
        let stale = page.as_of.is_some();
        self.count(channel, |tally| {
            tally.compared += 1;
            tally.stale += u64::from(stale);
            if !difference.is_empty() {
                tally.mismatched += 1;
                tally.id_mismatches += count(difference.ids.len());
                tally.hash_mismatches += count(difference.contents.len());
            }
        });
        if !difference.is_empty() {
            tracing::info!(
                channel = %channel,
                id_mismatches = difference.ids.len(),
                hash_mismatches = difference.contents.len(),
                stale,
                "recent view shadow: the Flat view disagreed with /messages"
            );
            tracing::debug!(
                channel = %channel,
                ids = ?difference.ids,
                contents = ?difference.contents,
                "recent view shadow: the messages that differed"
            );
        }
    }
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cell.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A running Flat read of one channel. Dropping it — the read finished, or its task was torn
/// down — lets the next `/messages` read of the channel start another.
struct Running {
    shadow: Arc<Shadow>,
    channel: ChannelId,
}

impl Running {
    fn claim(shadow: &Arc<Shadow>, channel: &ChannelId) -> Option<Self> {
        let claimed = lock(&shadow.running).insert(channel.clone());
        shadow.count(channel, |tally| {
            if claimed {
                tally.reads += 1;
            } else {
                tally.skipped += 1;
            }
        });
        claimed.then(|| Self {
            shadow: Arc::clone(shadow),
            channel: channel.clone(),
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        lock(&self.shadow.running).remove(&self.channel);
    }
}

/// Where a `/messages` read and a Flat read of the same channel disagree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Difference {
    /// Messages inside the stretch both reads cover that only one of them listed, oldest first.
    pub ids: Vec<MessageId>,
    /// Messages both listed whose content hashes differ, oldest first.
    pub contents: Vec<MessageId>,
}

impl Difference {
    /// Whether the two reads agreed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.contents.is_empty()
    }
}

/// Compare a `/messages` read with a Flat read of the same `limit`, both oldest first.
///
/// The stretch compared starts at the newer of the two reads' oldest messages — or at the
/// channel's start for a read that came back short, which claims the whole channel — and ends at
/// the newest message `messages` holds: a Flat message after that arrived after the `/messages`
/// read and is not a mismatch. An id that is not a number is always inside the stretch.
#[must_use]
pub fn compare(messages: &[Message], flat: &[Message], limit: u16) -> Difference {
    let floor = |read: &[Message]| {
        if read.len() < usize::from(limit) {
            0
        } else {
            read.iter()
                .filter_map(|m| m.id.numeric())
                .min()
                .unwrap_or(0)
        }
    };
    let lowest = floor(messages).max(floor(flat));
    let highest = messages
        .iter()
        .filter_map(|m| m.id.numeric())
        .max()
        .unwrap_or(u64::MAX);
    let inside = |m: &&Message| {
        m.id.numeric()
            .is_none_or(|n| (lowest..=highest).contains(&n))
    };
    let ours: HashMap<&MessageId, &Message> =
        messages.iter().filter(inside).map(|m| (&m.id, m)).collect();
    let theirs: HashMap<&MessageId, &Message> =
        flat.iter().filter(inside).map(|m| (&m.id, m)).collect();
    let mut difference = Difference::default();
    let mut seen: Vec<&Message> = ours.values().chain(theirs.values()).copied().collect();
    seen.sort_by_key(|m| (m.id.numeric().is_none(), m.id.numeric().unwrap_or(0)));
    seen.dedup_by(|later, earlier| later.id == earlier.id);
    for message in seen {
        match (ours.get(&message.id), theirs.get(&message.id)) {
            (Some(a), Some(b)) => {
                if crate::summarize::content_hash(&a.content)
                    != crate::summarize::content_hash(&b.content)
                {
                    difference.contents.push(message.id.clone());
                }
            }
            _ => difference.ids.push(message.id.clone()),
        }
    }
    difference
}

/// Every channel whose recent reads are shadowed, with its counts, for the diagnostics report.
/// Listed even before its first read, so the report shows the shadow is on.
#[must_use]
pub fn report(state: &AppState) -> Vec<ChannelTally> {
    state
        .all_channels()
        .into_iter()
        .filter(|channel| shadowed(state, &channel.id))
        .map(|channel| ChannelTally {
            tally: state.recent_view_shadow.tally(&channel.id),
            channel_id: channel.id,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UserId;

    fn message(n: u64, text: &str) -> Message {
        Message {
            thread: None,
            id: MessageId(n.to_string()),
            channel_id: ChannelId("1".to_owned()),
            author: "agent".to_owned(),
            author_id: UserId("9".to_owned()),
            author_is_bot: false,
            timestamp: String::new(),
            spoken_time: String::new(),
            reply_to: None,
            content: text.to_owned(),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            reactions: None,
            heading: None,
        }
    }

    fn ids(ids: &[u64]) -> Vec<MessageId> {
        ids.iter().map(|n| MessageId(n.to_string())).collect()
    }

    #[test]
    fn two_reads_of_the_same_messages_agree() {
        let read: Vec<Message> = (10..15).map(|n| message(n, "same")).collect();
        assert!(compare(&read, &read, 5).is_empty());
        assert!(compare(&[], &[], 5).is_empty());
    }

    #[test]
    fn a_message_only_one_read_listed_is_an_id_mismatch_and_changed_text_a_hash_mismatch() {
        let messages = [message(10, "a"), message(11, "b"), message(12, "c")];
        let flat = [message(10, "a"), message(12, "c, edited"), message(13, "d")];
        let difference = compare(&messages, &flat, 50);
        // 13 is newer than everything /messages returned: it arrived after that read.
        assert_eq!(difference.ids, ids(&[11]));
        assert_eq!(difference.contents, ids(&[12]));
    }

    #[test]
    fn only_the_stretch_both_full_reads_cover_is_compared() {
        // Both full, so each says nothing below its oldest message: Flat reaching one deeper is
        // not a mismatch. The newest message, which only /messages has, is.
        let messages: Vec<Message> = (11..=13).map(|n| message(n, "m")).collect();
        let flat: Vec<Message> = (10..=12).map(|n| message(n, "m")).collect();
        assert_eq!(compare(&messages, &flat, 3).ids, ids(&[13]));
        // A short read claims the whole channel, so what it lacks below the other's oldest is a
        // mismatch after all.
        let flat = [message(12, "m"), message(13, "m")];
        assert_eq!(compare(&messages, &flat, 4).ids, ids(&[11]));
    }

    #[test]
    fn a_flat_read_the_bridge_had_not_brought_up_to_date_shows_what_it_lacks() {
        let messages = [message(10, "a"), message(11, "b"), message(12, "newest")];
        let flat = [message(10, "a"), message(11, "b")];
        assert_eq!(compare(&messages, &flat, 50).ids, ids(&[12]));
    }

    #[test]
    fn the_counts_follow_each_comparison_and_failure() {
        let shadow = Shadow::new();
        let channel = ChannelId("1".to_owned());
        let messages = [message(10, "a"), message(11, "b")];
        let page = |messages: Vec<Message>, as_of: Option<&str>| TimelinePage {
            messages,
            as_of: as_of.map(str::to_owned),
            ..TimelinePage::default()
        };
        shadow.finish(&channel, &messages, 50, Ok(page(messages.to_vec(), None)));
        shadow.finish(
            &channel,
            &messages,
            50,
            Ok(page(
                vec![message(11, "b, edited")],
                Some("2026-10-12T10:00:00Z"),
            )),
        );
        shadow.finish(
            &channel,
            &messages,
            50,
            Err(ChatError::Transport("down".to_owned())),
        );
        assert_eq!(
            shadow.tally(&channel),
            Tally {
                reads: 0,
                skipped: 0,
                failed: 1,
                compared: 2,
                stale: 1,
                mismatched: 1,
                id_mismatches: 1,
                hash_mismatches: 1,
            }
        );
    }

    #[test]
    fn one_flat_read_runs_per_channel_and_the_next_waits_for_it_to_end() {
        let shadow = Arc::new(Shadow::new());
        let channel = ChannelId("1".to_owned());
        let first = Running::claim(&shadow, &channel).expect("the first read starts");
        assert!(Running::claim(&shadow, &channel).is_none());
        assert!(Running::claim(&shadow, &ChannelId("2".to_owned())).is_some());
        drop(first);
        assert!(Running::claim(&shadow, &channel).is_some());
        let tally = shadow.tally(&channel);
        assert_eq!((tally.reads, tally.skipped), (2, 1));
    }
}
