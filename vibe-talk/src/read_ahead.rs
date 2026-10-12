//! What speech preparation keeps between requests, so a tap does not wait for a fetch the page
//! has only just made. `#191 read-aloud-first-lag`.
//!
//! The measured first read of a session took five seconds from tap to sound, and 1.78 s of that
//! was the tap waiting for `POST /api/v1/channels/{id}/speech/prepare`. Nearly all of the prepare
//! was one chat window fetch, which re-read the window the page itself had fetched a few seconds
//! earlier. In a threaded view it was worse: the prepare fetched each message on its own, one
//! after the other. And the provider warm-up waited for all of it before it started.
//!
//! This module holds three things that remove those waits:
//!
//! - [`ReadAhead::remember`] and [`ReadAhead::recall`]: every message this server fetches for a
//!   page (`ops::messages`, `ops::page`, `ops::timeline`) is kept for [`RECENT_TTL`], and
//!   preparation looks there first. Bounded by [`MAX_RECENT`].
//! - [`ReadAhead::window_turn`]: when several preparations for one channel miss together, one of
//!   them fetches the window and the others wait for it, rather than queueing a second identical
//!   fetch behind the first at the chat adapter.
//! - [`ReadAhead::start_warm_up`]: the provider warm-up starts at the top of a preparation and is
//!   never awaited by it. At most one runs at a time.
//!
//! [`messages_for_speech`] puts the first two together for the prepare route.
//!
//! # What is kept, and for how long
//!
//! Message text from allowlisted channels, in memory only, for thirty seconds. That is the same
//! text the page was handed in the same thirty seconds. Nothing here reaches the store, and a
//! restart forgets it. A read still checks the channel allowlist first (the prepare route does
//! that before it calls [`messages_for_speech`]), so a channel taken off the list cannot be read
//! from here either. An update or delete pushed by an adapter, and a delta that reports a
//! deletion, drop the message at once.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{StreamExt as _, TryStreamExt as _};

use crate::model::{ChannelId, ChannelInfo, Message, MessageId};
use crate::ops::{self, OpError};
use crate::speech::SpeechProvider;
use crate::state::AppState;

/// How long a fetched message stays readable from here.
///
/// Long enough to cover "the page drew the channel, the reader turned Read on and tapped a line".
/// The measured case was 3 to 8 seconds. Short enough that an edit nobody pushed is stale for
/// half a minute at most. A ticket already minted keeps its text for longer than this anyway.
pub const RECENT_TTL: Duration = Duration::from_secs(30);

/// The most messages kept at once, across every channel.
///
/// Ten full windows of a hundred. Beyond it the oldest are dropped first.
pub const MAX_RECENT: usize = 1_000;

/// How many thread messages one preparation fetches at once when they missed.
///
/// Measured against the deployed chat adapter's own log (completion times only). A lookup the
/// adapter answers from its own cache took about 42 ms when the previous prepare route asked for
/// them one after another. Lookups that were in flight together completed 0 to 9 ms apart. So the
/// per-lookup cost is mostly round trip, and a few in flight together removes most of it. A lookup
/// the adapter must send upstream still waits for its single upstream slot, and four in flight
/// only queue there. They add no upstream load.
pub const THREAD_FETCH_CONCURRENCY: usize = 4;

#[derive(Debug)]
struct Recent {
    message: Message,
    /// The thread the backend fetched this under or labelled it with. `None` is the channel's
    /// ordinary window.
    thread: Option<String>,
    fetched: Instant,
}

/// Messages fetched in the last [`RECENT_TTL`], one window fetch per channel at a time, and the
/// provider warm-up. See the module documentation.
#[derive(Debug, Default)]
pub struct ReadAhead {
    recent: Mutex<HashMap<(ChannelId, MessageId), Recent>>,
    /// Per channel: when the last window fetch made through [`ReadAhead::window_turn`] finished.
    windows: Mutex<HashMap<ChannelId, Arc<tokio::sync::Mutex<Option<Instant>>>>>,
    warming: Arc<AtomicBool>,
}

impl ReadAhead {
    /// Nothing remembered.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep `messages`, just fetched from `channel`.
    ///
    /// `thread` is the thread the fetch was scoped to. Without one, each message is filed under
    /// the thread the backend labelled it with, if any. A thread lookup must match that label
    /// before it is answered from here.
    pub fn remember(&self, channel: &ChannelId, thread: Option<&str>, messages: &[Message]) {
        self.remember_at(channel, thread, messages, Instant::now());
    }

    fn remember_at(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        messages: &[Message],
        now: Instant,
    ) {
        if messages.is_empty() {
            return;
        }
        let Ok(mut held) = self.recent.lock() else {
            return;
        };
        held.retain(|_, entry| now.saturating_duration_since(entry.fetched) < RECENT_TTL);
        for message in messages {
            let filed_under = thread
                .map(str::to_owned)
                .or_else(|| message.thread.as_ref().map(|t| t.id.clone()));
            held.insert(
                (channel.clone(), message.id.clone()),
                Recent {
                    message: message.clone(),
                    thread: filed_under,
                    fetched: now,
                },
            );
        }
        if held.len() > MAX_RECENT {
            let mut ages: Vec<_> = held
                .iter()
                .map(|(key, entry)| (entry.fetched, key.clone()))
                .collect();
            ages.sort_by_key(|(fetched, _)| *fetched);
            let excess = held.len() - MAX_RECENT;
            for (_, key) in ages.into_iter().take(excess) {
                held.remove(&key);
            }
        }
    }

    /// The messages among `ids` that were fetched from `channel` in the last [`RECENT_TTL`].
    ///
    /// With `thread`, only a message fetched in that thread or labelled with it. Without one, any
    /// message of the channel: the channel allowlist is the boundary, and the caller has already
    /// checked it.
    #[must_use]
    pub fn recall(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        ids: &[String],
    ) -> HashMap<String, Message> {
        self.recall_at(channel, thread, ids, Instant::now())
    }

    fn recall_at(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        ids: &[String],
        now: Instant,
    ) -> HashMap<String, Message> {
        let Ok(held) = self.recent.lock() else {
            return HashMap::new();
        };
        let mut found = HashMap::new();
        for id in ids {
            let Some(entry) = held.get(&(channel.clone(), MessageId(id.clone()))) else {
                continue;
            };
            if now.saturating_duration_since(entry.fetched) >= RECENT_TTL {
                continue;
            }
            if thread.is_some_and(|thread| entry.thread.as_deref() != Some(thread)) {
                continue;
            }
            found.insert(id.clone(), entry.message.clone());
        }
        found
    }

    /// Drop one message: it was edited or deleted, and the copy here no longer says what it says.
    pub fn forget(&self, channel: &ChannelId, id: &MessageId) {
        if let Ok(mut held) = self.recent.lock() {
            held.remove(&(channel.clone(), id.clone()));
        }
    }

    /// Drop everything. For the storage purge, which promises that nothing derived from a
    /// channel is left behind.
    pub fn clear(&self) {
        if let Ok(mut held) = self.recent.lock() {
            held.clear();
        }
    }

    /// How many messages are held, expired ones included until the next [`ReadAhead::remember`].
    #[must_use]
    pub fn len(&self) -> usize {
        self.recent.lock().map_or(0, |held| held.len())
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Wait for this channel's window fetch slot.
    ///
    /// The guard holds when the last window fetch through this slot finished. A caller that finds
    /// a time later than its own start waited behind a fetch that has just read the window, and
    /// should use that rather than read it again.
    pub async fn window_turn(
        &self,
        channel: &ChannelId,
    ) -> tokio::sync::OwnedMutexGuard<Option<Instant>> {
        let slot = {
            let mut windows = self
                .windows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(windows.entry(channel.clone()).or_default())
        };
        slot.lock_owned().await
    }

    /// Start `speech`'s warm-up in the background, unless one is already running.
    ///
    /// Answers whether this call started one. The caller does not wait for it. A tap that comes
    /// while the warm-up is still opening a session joins that opening (see
    /// `ConversationSpeech::session_for_read`), so there is nothing to gain by holding the prepare
    /// response for it. A failure is logged, and the read itself reports it if it persists.
    pub fn start_warm_up(&self, speech: Arc<dyn SpeechProvider>) -> bool {
        if self.warming.swap(true, Ordering::AcqRel) {
            return false;
        }
        let running = WarmingFlag(Arc::clone(&self.warming));
        tokio::spawn(async move {
            // Held across the await, so the flag clears even if the provider panics.
            let _running = running;
            let began = Instant::now();
            let outcome = speech.warm_up().await;
            tracing::info!(
                warm_up_ms = millis(began.elapsed()),
                ok = outcome.is_ok(),
                "read-aloud warm-up finished"
            );
        });
        true
    }

    /// Whether a warm-up started by [`ReadAhead::start_warm_up`] is still running.
    #[must_use]
    pub fn warming(&self) -> bool {
        self.warming.load(Ordering::Acquire)
    }
}

/// Clears the warm-up flag when the warm-up task ends, however it ends.
struct WarmingFlag(Arc<AtomicBool>);

impl Drop for WarmingFlag {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The messages a preparation asked for, and what finding them took.
#[derive(Debug, Default)]
pub struct Found {
    /// Message id to message, for every id that was found.
    pub messages: HashMap<String, Message>,
    /// How many distinct ids were answered from [`ReadAhead`] without a fetch.
    pub hits: usize,
    /// How many chat fetches this preparation made: one per window, one per thread message.
    pub fetches: usize,
    /// Whether this preparation waited for another one's window fetch instead of making its own.
    pub joined: bool,
    /// Time spent looking in [`ReadAhead`].
    pub lookup: Duration,
    /// Time spent fetching, or waiting for another preparation's fetch.
    pub fetch: Duration,
}

/// Find `ids` of `channel` for speech: from what was fetched recently, and fetch only what is
/// missing.
///
/// Without a thread, the missing ids cost one window fetch, shared with any other preparation of
/// the same channel already making one. An id that is not in the window is simply absent, as it
/// always was. In a thread, each missing id is fetched on its own, up to
/// [`THREAD_FETCH_CONCURRENCY`] at once, and the first failure fails the batch, as it did when
/// they were fetched in series.
///
/// `began` is when the preparation started. A window fetch that finished after it is fresh
/// enough to answer this one.
///
/// The caller has already checked `channel` against the allowlist.
///
/// # Errors
///
/// The errors of [`ops::messages`] and [`ops::message_by_id_scoped`].
pub async fn messages_for_speech(
    state: &AppState,
    channel: &ChannelInfo,
    thread: Option<&str>,
    ids: &[String],
    limit: Option<u16>,
    began: Instant,
) -> Result<Found, OpError> {
    let looking = Instant::now();
    let mut found = Found {
        messages: state.read_ahead.recall(&channel.id, thread, ids),
        ..Found::default()
    };
    found.hits = found.messages.len();
    found.lookup = looking.elapsed();
    let mut seen = HashSet::new();
    let missing: Vec<String> = ids
        .iter()
        .filter(|id| !found.messages.contains_key(*id) && seen.insert(id.as_str()))
        .cloned()
        .collect();
    if missing.is_empty() {
        return Ok(found);
    }
    let fetching = Instant::now();
    match thread {
        None => {
            let mut turn = state.read_ahead.window_turn(&channel.id).await;
            if turn.is_some_and(|finished| finished >= began) {
                found.joined = true;
                found
                    .messages
                    .extend(state.read_ahead.recall(&channel.id, None, &missing));
            } else {
                // `ops::messages` remembers the window for everyone else as well.
                let window = ops::messages(state, channel.id.as_str(), limit).await?;
                *turn = Some(Instant::now());
                found.fetches = 1;
                let wanted: HashSet<&str> = missing.iter().map(String::as_str).collect();
                for message in window.messages {
                    if wanted.contains(message.id.as_str()) {
                        found.messages.insert(message.id.0.clone(), message);
                    }
                }
            }
        }
        Some(thread) => {
            let fetched: Vec<Message> = futures_util::stream::iter(missing)
                .map(|id| async move {
                    ops::message_by_id_scoped(state, channel.id.as_str(), &id, limit, Some(thread))
                        .await
                        .map(|(_, message)| message)
                })
                .buffered(THREAD_FETCH_CONCURRENCY)
                .try_collect()
                .await?;
            found.fetches = fetched.len();
            state
                .read_ahead
                .remember(&channel.id, Some(thread), &fetched);
            for message in fetched {
                found.messages.insert(message.id.0.clone(), message);
            }
        }
    }
    found.fetch = fetching.elapsed();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UserId;
    use crate::threads::MessageThread;

    fn message(id: &str, thread: Option<&str>) -> Message {
        Message {
            id: MessageId(id.to_owned()),
            channel_id: ChannelId("1".to_owned()),
            author: "Reader".to_owned(),
            author_id: UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: "2026-10-12T01:00:00Z".to_owned(),
            spoken_time: String::new(),
            reply_to: None,
            content: format!("text of {id}"),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            thread: thread.map(|id| MessageThread {
                id: id.to_owned(),
                root_message_id: None,
                is_root: false,
                reply_count: None,
                reply_count_exact: false,
            }),
            reactions: None,
            heading: None,
        }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn a_message_is_recalled_until_it_expires() {
        let read_ahead = ReadAhead::new();
        let channel = ChannelId("1".to_owned());
        let then = Instant::now();
        read_ahead.remember_at(&channel, None, &[message("10", None)], then);
        let found = read_ahead.recall_at(&channel, None, &ids(&["10", "11"]), then);
        assert_eq!(found.len(), 1);
        assert_eq!(found["10"].content, "text of 10");
        let later = then + RECENT_TTL - Duration::from_millis(1);
        assert_eq!(
            read_ahead
                .recall_at(&channel, None, &ids(&["10"]), later)
                .len(),
            1
        );
        let expired = then + RECENT_TTL;
        assert!(read_ahead
            .recall_at(&channel, None, &ids(&["10"]), expired)
            .is_empty());
    }

    #[test]
    fn another_channel_never_answers() {
        let read_ahead = ReadAhead::new();
        read_ahead.remember(&ChannelId("1".to_owned()), None, &[message("10", None)]);
        assert!(read_ahead
            .recall(&ChannelId("2".to_owned()), None, &ids(&["10"]))
            .is_empty());
    }

    #[test]
    fn a_thread_lookup_needs_the_thread_the_message_was_fetched_in() {
        let read_ahead = ReadAhead::new();
        let channel = ChannelId("1".to_owned());
        // Labelled by the backend in an unscoped (flat) read.
        read_ahead.remember(&channel, None, &[message("10", Some("T"))]);
        // Fetched in a thread-scoped read with no label of its own.
        read_ahead.remember(&channel, Some("U"), &[message("20", None)]);
        // Neither thread, nor labelled.
        read_ahead.remember(&channel, None, &[message("30", None)]);
        let wanted = ids(&["10", "20", "30"]);
        let in_t = read_ahead.recall(&channel, Some("T"), &wanted);
        assert_eq!(in_t.keys().collect::<Vec<_>>(), vec!["10"]);
        let in_u = read_ahead.recall(&channel, Some("U"), &wanted);
        assert_eq!(in_u.keys().collect::<Vec<_>>(), vec!["20"]);
        assert!(read_ahead
            .recall(&channel, Some("elsewhere"), &wanted)
            .is_empty());
        assert_eq!(read_ahead.recall(&channel, None, &wanted).len(), 3);
    }

    #[test]
    fn forgetting_an_edited_message_drops_only_that_one() {
        let read_ahead = ReadAhead::new();
        let channel = ChannelId("1".to_owned());
        read_ahead.remember(&channel, None, &[message("10", None), message("11", None)]);
        read_ahead.forget(&channel, &MessageId("10".to_owned()));
        let found = read_ahead.recall(&channel, None, &ids(&["10", "11"]));
        assert_eq!(found.keys().collect::<Vec<_>>(), vec!["11"]);
        read_ahead.clear();
        assert!(read_ahead.is_empty());
    }

    #[test]
    fn the_oldest_go_first_beyond_the_bound() {
        let read_ahead = ReadAhead::new();
        let channel = ChannelId("1".to_owned());
        let start = Instant::now();
        let first: Vec<Message> = (0..MAX_RECENT)
            .map(|n| message(&format!("a{n}"), None))
            .collect();
        read_ahead.remember_at(&channel, None, &first, start);
        let newer = start + Duration::from_millis(5);
        read_ahead.remember_at(
            &channel,
            None,
            &[message("b0", None), message("b1", None)],
            newer,
        );
        assert_eq!(read_ahead.len(), MAX_RECENT);
        let found = read_ahead.recall_at(&channel, None, &ids(&["b0", "b1"]), newer);
        assert_eq!(found.len(), 2, "the newest were the ones dropped");
    }

    #[test]
    fn expired_messages_are_dropped_when_more_arrive() {
        let read_ahead = ReadAhead::new();
        let channel = ChannelId("1".to_owned());
        let start = Instant::now();
        read_ahead.remember_at(&channel, None, &[message("10", None)], start);
        read_ahead.remember_at(&channel, None, &[message("11", None)], start + RECENT_TTL);
        assert_eq!(read_ahead.len(), 1);
    }
}
