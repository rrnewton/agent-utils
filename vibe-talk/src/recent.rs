//! Each channel's most recent window, held in memory, so a catch-up can answer inside a voice
//! agent's tool limit while the chat service is slow. `#210 voice-catchup-speed`.
//!
//! # Why this exists
//!
//! `recent_activity` read every listed channel from the chat service on every call. Behind an
//! adapter that runs one provider command at a time, three channels cost three reads in series —
//! about two seconds each — so a catch-up took five to nine seconds, and the one slow command that
//! stalls for a minute takes every tool down with it. The voice runtime gives a tool fifteen
//! seconds, and a call that runs past that is answered with nothing at all. More parallelism is
//! not available: the adapter's single slot is a resource limit, not an oversight.
//!
//! What the server can do on its own is not wait for the slowest read. It keeps the newest window
//! it has read of each channel, keeps that window current from the live feed, and gives a catch-up
//! a deadline: whatever has not been read again by then is answered from the held copy, labelled
//! with its age, rather than not at all.
//!
//! See also [`crate::read_ahead`], read-aloud's separate 30-second memory of fetched messages.
//!
//! # What is held, and for how long
//!
//! For each channel, the RAW messages of the newest successful read — any
//! [`crate::ops::messages`] read, from a tool or from the page, or a refresh started here — and
//! when that read began. Every create, edit and delete the [`crate::live::LiveHub`] publishes is
//! merged in, so a window read a minute ago and fed since is as current as the feed. The
//! spoken forms and the noise marks are NOT held: they are worked out again each time a window is
//! served, exactly as for a fresh read, so a held copy can never carry a stale classification.
//!
//! Memory only. Nothing here is written to disk, it is gone when the process restarts, and it is
//! bounded by the channel allowlist times one read's size: every path that records or merges a
//! window is already allowlist-gated.
//!
//! # One read per channel at a time
//!
//! A refresh is SINGLE-FLIGHT: while one channel is being read, a second caller waits on the same
//! read instead of queueing another behind it, and the read is a detached task, so it finishes —
//! and fills the window — even when the caller that started it has given up. That is what makes
//! the retry after an abandoned call cheap: the read it would have repeated has already landed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use crate::chat::ChatError;
use crate::live::{LiveKind, LiveMessage};
use crate::model::{ChannelId, Message};
use crate::state::AppState;

/// How long a catch-up waits for the channels it holds a copy of. Measured catch-ups took five to
/// nine seconds against a fifteen-second tool limit; this leaves room for the speech model too.
pub const ACTIVITY_DEADLINE: Duration = Duration::from_millis(2_500);

/// How long a catch-up waits for a channel it holds NO copy of, before saying it could not be read
/// in time. Longer, because the alternative is an answer with nothing in it, and still well inside
/// the fifteen-second tool limit.
pub const COLD_DEADLINE: Duration = Duration::from_secs(8);

/// How old a window may be and still be served without reading the channel again, when a live
/// producer is feeding it: every change since the read has been merged in.
pub const REUSE_LIVE_FED: Duration = Duration::from_secs(60);

/// The same, when nothing feeds the window. Long enough that the read started when a call begins
/// is the one its first catch-up uses — measured at five to six seconds apart — and short enough
/// that a second catch-up in the same conversation reads again.
pub const REUSE_UNFED: Duration = Duration::from_secs(15);

/// How many refreshes read at once, across every channel. Four, as the catch-up's fan-out always
/// was: enough that three channels cost one round trip against a provider that can take them,
/// few enough that a long channel list does not burst a provider's rate limit.
pub const REFRESH_CONCURRENCY: usize = 4;

/// The four durations, gathered so a test can shorten them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// See [`ACTIVITY_DEADLINE`].
    pub deadline: Duration,
    /// See [`COLD_DEADLINE`].
    pub cold_deadline: Duration,
    /// See [`REUSE_LIVE_FED`].
    pub reuse_live_fed: Duration,
    /// See [`REUSE_UNFED`].
    pub reuse_unfed: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            deadline: ACTIVITY_DEADLINE,
            cold_deadline: COLD_DEADLINE,
            reuse_live_fed: REUSE_LIVE_FED,
            reuse_unfed: REUSE_UNFED,
        }
    }
}

/// One channel's held window.
#[derive(Clone, Debug)]
pub struct Held {
    /// Raw messages, oldest first: never stamped, never noise-marked.
    pub messages: Vec<Message>,
    /// The size the read asked for.
    pub limit: u16,
    /// Whether `messages` is the whole channel: the read came back short, and nothing since has
    /// pushed the oldest message out.
    pub whole: bool,
    /// When the read that produced this window began. Its age is counted from here, because
    /// the window says nothing about the channel before that moment.
    pub read_started: Instant,
    /// When that read finished.
    pub read_finished: Instant,
}

impl Held {
    /// How long ago the read that produced this window began: the age to SAY, since the window
    /// may know nothing of the channel after that moment.
    #[must_use]
    pub fn age(&self) -> Duration {
        self.read_started.elapsed()
    }

    /// How long ago that read finished: the age that decides whether to read again. A read that
    /// waited a minute for the provider's slot is not a minute stale when it lands.
    #[must_use]
    pub fn since_finished(&self) -> Duration {
        self.read_finished.elapsed()
    }
}

/// What this server knows about one channel right now.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// The held window, if any read has succeeded.
    pub held: Option<Held>,
    /// Whether a refresh is running.
    pub refreshing: bool,
    /// Why the newest refresh failed, when it did and no read has succeeded since.
    pub failure: Option<ChatError>,
}

#[derive(Default)]
struct Slot {
    held: Option<Held>,
    /// The running refresh, and its number, so only that refresh can clear it.
    refreshing: Option<(u64, watch::Receiver<bool>)>,
    /// The newest refresh failure and when its read began. Cleared by a later success.
    failure: Option<(ChatError, Instant)>,
}

/// Every channel's held window, and the refreshes running for them.
pub struct RecentWindows {
    timing: Timing,
    slots: Mutex<BTreeMap<ChannelId, Slot>>,
    next_refresh: std::sync::atomic::AtomicU64,
    /// [`REFRESH_CONCURRENCY`] permits; a refresh holds one while it reads.
    reading: tokio::sync::Semaphore,
}

impl Default for RecentWindows {
    fn default() -> Self {
        Self::with_timing(Timing::default())
    }
}

impl RecentWindows {
    /// Nothing held, with the production durations.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Nothing held, with these durations. For tests.
    #[must_use]
    pub fn with_timing(timing: Timing) -> Self {
        Self {
            timing,
            slots: Mutex::new(BTreeMap::new()),
            next_refresh: std::sync::atomic::AtomicU64::new(1),
            reading: tokio::sync::Semaphore::new(REFRESH_CONCURRENCY),
        }
    }

    /// The durations in force.
    #[must_use]
    pub fn timing(&self) -> Timing {
        self.timing
    }

    fn slots(&self) -> std::sync::MutexGuard<'_, BTreeMap<ChannelId, Slot>> {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keep the result of a successful read of `channel`, which began at `started`.
    ///
    /// A read that began before the one already held is dropped: two reads overlapping, the one
    /// that started later saw at least as much of the channel, whichever finished first.
    pub fn record(&self, channel: &ChannelId, messages: &[Message], limit: u16, started: Instant) {
        let mut slots = self.slots();
        let slot = slots.entry(channel.clone()).or_default();
        if slot
            .failure
            .as_ref()
            .is_some_and(|(_, failed)| *failed <= started)
        {
            slot.failure = None;
        }
        if slot
            .held
            .as_ref()
            .is_some_and(|held| held.read_started > started)
        {
            return;
        }
        slot.held = Some(Held {
            messages: messages.to_vec(),
            limit,
            whole: messages.len() < usize::from(limit),
            read_started: started,
            read_finished: Instant::now(),
        });
    }

    fn note_failure(&self, channel: &ChannelId, error: ChatError, started: Instant) {
        let mut slots = self.slots();
        let slot = slots.entry(channel.clone()).or_default();
        if slot
            .held
            .as_ref()
            .is_some_and(|held| held.read_started > started)
        {
            return;
        }
        slot.failure = Some((error, started));
    }

    /// Fold one published change into `channel`'s window, when one is held.
    ///
    /// A push never STARTS a window: one message says nothing about what came before it.
    pub fn merge(&self, channel: &ChannelId, live: &LiveMessage) {
        let mut slots = self.slots();
        let Some(held) = slots.get_mut(channel).and_then(|slot| slot.held.as_mut()) else {
            return;
        };
        match live.kind {
            LiveKind::Delete => held.messages.retain(|m| m.id != live.message.id),
            LiveKind::Update => {
                if let Some(at) = held.messages.iter().position(|m| m.id == live.message.id) {
                    held.messages[at] = merged(&held.messages[at], &live.message);
                }
            }
            LiveKind::Create => insert(held, &live.message),
        }
    }

    /// What is known about `channel` right now.
    #[must_use]
    pub fn snapshot(&self, channel: &ChannelId) -> Snapshot {
        let slots = self.slots();
        let Some(slot) = slots.get(channel) else {
            return Snapshot::default();
        };
        Snapshot {
            held: slot.held.clone(),
            refreshing: slot.refreshing.is_some(),
            failure: slot.failure.as_ref().map(|(error, _)| error.clone()),
        }
    }

    /// Whether `channel` holds a window at all.
    #[must_use]
    pub fn holds(&self, channel: &ChannelId) -> bool {
        self.slots()
            .get(channel)
            .is_some_and(|slot| slot.held.is_some())
    }

    /// Whether `channel`'s window can answer a read of `scan` messages without asking the chat
    /// service: its read finished within `reuse`, and it reaches as deep as asked or holds the
    /// whole channel.
    #[must_use]
    pub fn reusable(&self, channel: &ChannelId, scan: u16, reuse: Duration) -> bool {
        self.slots()
            .get(channel)
            .and_then(|slot| slot.held.as_ref())
            .is_some_and(|held| {
                held.since_finished() <= reuse && (held.limit >= scan || held.whole)
            })
    }

    /// Join the refresh running for `channel`, or claim the right to start one.
    ///
    /// The receiver turns `true` when the refresh is over, however it ended. A claim is returned
    /// only to the caller that must run the read; every other caller is handed the same receiver.
    fn claim(self: &Arc<Self>, channel: &ChannelId) -> (watch::Receiver<bool>, Option<Claim>) {
        let mut slots = self.slots();
        let slot = slots.entry(channel.clone()).or_default();
        if let Some((_, running)) = &slot.refreshing {
            return (running.clone(), None);
        }
        let number = self
            .next_refresh
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (done, running) = watch::channel(false);
        slot.refreshing = Some((number, running.clone()));
        let claim = Claim {
            windows: Arc::clone(self),
            channel: channel.clone(),
            number,
            done: Some(done),
        };
        (running, Some(claim))
    }

    fn finish_refresh(&self, channel: &ChannelId, number: u64) {
        let mut slots = self.slots();
        if let Some(slot) = slots.get_mut(channel) {
            if slot
                .refreshing
                .as_ref()
                .is_some_and(|(running, _)| *running == number)
            {
                slot.refreshing = None;
            }
        }
    }
}

/// The right, and the duty, to run one channel's refresh. Dropping it unfinished — a task torn
/// down with the runtime — still clears the running mark, so a channel cannot be left waiting on
/// a refresh that will never end.
struct Claim {
    windows: Arc<RecentWindows>,
    channel: ChannelId,
    number: u64,
    done: Option<watch::Sender<bool>>,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.windows.finish_refresh(&self.channel, self.number);
        if let Some(done) = self.done.take() {
            let _ = done.send(true);
        }
    }
}

/// The message a live edit leaves behind: the edit, keeping what an echo cannot see.
///
/// A live copy carries no reactions and may carry no thread; neither absence means "removed", so
/// the held copy's survive. See [`Message::reactions`].
fn merged(held: &Message, edit: &Message) -> Message {
    let mut next = edit.clone();
    if next.reactions.is_none() {
        next.reactions.clone_from(&held.reactions);
    }
    if next.thread.is_none() {
        next.thread.clone_from(&held.thread);
    }
    next
}

/// When a message was created, for placing it in a window: from its id where that carries a time,
/// otherwise from its timestamp.
fn created_ms(message: &Message) -> Option<i64> {
    message
        .id
        .created_at_ms()
        .or_else(|| crate::clock::instant_ms(&message.timestamp))
}

/// Place one created message in a window, oldest first.
fn insert(held: &mut Held, message: &Message) {
    if let Some(at) = held.messages.iter().position(|m| m.id == message.id) {
        // An echo of a message the read already brought.
        held.messages[at] = merged(&held.messages[at], message);
        return;
    }
    let position = match created_ms(message) {
        Some(at) => held
            .messages
            .iter()
            .rposition(|m| created_ms(m).is_some_and(|ms| ms <= at))
            .map_or(0, |found| found + 1),
        None => held.messages.len(),
    };
    // Older than the oldest message of a window that is not the whole channel: it belongs to a
    // stretch the window never read, and keeping it would make the window claim that stretch.
    if position == 0 && !held.whole && !held.messages.is_empty() {
        return;
    }
    held.messages.insert(position, message.clone());
    if held.messages.len() > usize::from(held.limit) {
        held.messages.remove(0);
        held.whole = false;
    }
}

/// How young `channel`'s window must be to be served without reading it again.
#[must_use]
pub fn reuse_for(state: &AppState, channel: &ChannelId) -> Duration {
    let timing = state.live.recent().timing();
    let polled = state
        .providers
        .key_for(channel)
        .as_deref()
        .and_then(|key| state.providers.entry(key))
        .is_some_and(|entry| entry.live_poll_seconds > 0);
    if polled || state.config.ingest.enabled() {
        timing.reuse_live_fed
    } else {
        timing.reuse_unfed
    }
}

/// Start reading `channel`'s newest `limit` messages, or join the read already running.
///
/// The read is a detached task: it ends, and records what it read, whatever happens to the
/// caller. The receiver turns `true` when it is over.
pub fn refresh(state: &AppState, channel: &ChannelId, limit: u16) -> watch::Receiver<bool> {
    let windows = Arc::clone(state.live.recent());
    let (done, claim) = windows.claim(channel);
    if let Some(claim) = claim {
        let state = state.clone();
        tokio::spawn(async move {
            // Begun when asked for, not when a permit came free: the window is no newer than that.
            let started = Instant::now();
            let read = match claim.windows.reading.acquire().await {
                Ok(_permit) => state.chat.fetch_recent(&claim.channel, limit).await,
                Err(_) => Err(ChatError::Refused("the server is shutting down".to_owned())),
            };
            let millis = started.elapsed().as_millis();
            match read {
                Ok(messages) => {
                    tracing::info!(
                        channel = %claim.channel,
                        returned = messages.len(),
                        millis,
                        "recent window refreshed"
                    );
                    claim
                        .windows
                        .record(&claim.channel, &messages, limit, started);
                }
                Err(error) => {
                    tracing::warn!(
                        channel = %claim.channel,
                        millis,
                        %error,
                        "recent window refresh failed"
                    );
                    claim.windows.note_failure(&claim.channel, error, started);
                }
            }
            drop(claim);
        });
    }
    done
}

/// Start reading every listed channel that a catch-up would read again, and return at once with
/// how many are being read.
///
/// For the moment a voice call is being set up: the greeting takes five to six seconds before
/// the agent can ask anything, and that is time for the reads its first catch-up will want.
/// Single-flight like every refresh, and a window young enough to reuse is left alone, so a burst
/// of call starts does not stack reads.
pub fn warm(state: &AppState) -> usize {
    let scan = crate::ops::activity_scan(state, crate::ops::DEFAULT_ACTIVITY_LINES);
    let recent = state.live.recent();
    let mut reading = 0;
    for channel in state.all_channels() {
        if recent.reusable(&channel.id, scan, reuse_for(state, &channel.id)) {
            continue;
        }
        drop(refresh(state, &channel.id, scan));
        reading += 1;
    }
    reading
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MessageId, UserId};

    fn message(id: u64, text: &str) -> Message {
        Message {
            thread: None,
            id: MessageId(id.to_string()),
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

    fn live(kind: LiveKind, message: Message) -> LiveMessage {
        LiveMessage {
            event_id: message.id.as_str().to_owned(),
            kind,
            historical: false,
            message,
            self_posted: false,
        }
    }

    fn ids(windows: &RecentWindows, channel: &ChannelId) -> Vec<String> {
        windows
            .snapshot(channel)
            .held
            .expect("held")
            .messages
            .iter()
            .map(|m| m.id.as_str().to_owned())
            .collect()
    }

    // Snowflake ids a thousand apart in their time bits, so their order is their creation order.
    const BASE: u64 = 1 << 40;
    fn id(n: u64) -> u64 {
        BASE + (n << 22)
    }

    #[tokio::test]
    async fn pushes_keep_a_held_window_current_and_never_start_one() {
        let windows = RecentWindows::new();
        let channel = ChannelId("1".to_owned());
        windows.merge(&channel, &live(LiveKind::Create, message(id(1), "early")));
        assert!(!windows.holds(&channel), "a push started a window");

        let read = [message(id(1), "one"), message(id(2), "two")];
        windows.record(&channel, &read, 3, Instant::now());
        assert!(windows.snapshot(&channel).held.expect("held").whole);

        windows.merge(&channel, &live(LiveKind::Create, message(id(3), "three")));
        windows.merge(
            &channel,
            &live(LiveKind::Update, message(id(2), "two, edited")),
        );
        windows.merge(&channel, &live(LiveKind::Delete, message(id(1), "")));
        let held = windows.snapshot(&channel).held.expect("held");
        assert_eq!(
            held.messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            ["two, edited", "three"]
        );

        // Two more push the window past its size: the oldest goes, and it is no longer whole.
        windows.merge(&channel, &live(LiveKind::Create, message(id(4), "four")));
        windows.merge(&channel, &live(LiveKind::Create, message(id(5), "five")));
        assert_eq!(
            ids(&windows, &channel),
            [id(3), id(4), id(5)].map(|n| n.to_string())
        );
        assert!(!windows.snapshot(&channel).held.expect("held").whole);

        // A late message older than a partial window's start is kept out; one inside it is placed.
        windows.merge(&channel, &live(LiveKind::Create, message(id(2), "late")));
        windows.merge(
            &channel,
            &live(LiveKind::Create, message(id(4) + 1, "between")),
        );
        assert_eq!(
            ids(&windows, &channel),
            [id(4), id(4) + 1, id(5)].map(|n| n.to_string())
        );
    }

    #[tokio::test]
    async fn an_echo_keeps_the_reactions_the_read_brought() {
        let windows = RecentWindows::new();
        let channel = ChannelId("1".to_owned());
        let mut read = message(id(1), "one");
        read.reactions = Some(Vec::new());
        windows.record(&channel, &[read], 50, Instant::now());
        windows.merge(&channel, &live(LiveKind::Create, message(id(1), "one")));
        windows.merge(&channel, &live(LiveKind::Update, message(id(1), "one!")));
        let held = windows.snapshot(&channel).held.expect("held");
        assert_eq!(held.messages.len(), 1);
        assert_eq!(held.messages[0].content, "one!");
        assert_eq!(held.messages[0].reactions, Some(Vec::new()));
    }

    #[tokio::test(start_paused = true)]
    async fn an_older_read_finishing_late_does_not_replace_a_newer_one() {
        let windows = RecentWindows::new();
        let channel = ChannelId("1".to_owned());
        let early = Instant::now();
        tokio::time::sleep(Duration::from_secs(1)).await;
        windows.record(&channel, &[message(id(2), "newer")], 50, Instant::now());
        windows.record(&channel, &[message(id(1), "older")], 50, early);
        assert_eq!(ids(&windows, &channel), [id(2).to_string()]);

        windows.note_failure(&channel, ChatError::Transport("late".to_owned()), early);
        assert!(
            windows.snapshot(&channel).failure.is_none(),
            "a failure of a read older than the held window is not news"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_window_is_reused_only_while_young_and_deep_enough() {
        let windows = RecentWindows::new();
        let channel = ChannelId("1".to_owned());
        let full: Vec<Message> = (1..=20).map(|n| message(id(n), "m")).collect();
        windows.record(&channel, &full, 20, Instant::now());
        let reuse = Duration::from_secs(15);
        assert!(windows.reusable(&channel, 20, reuse));
        assert!(
            !windows.reusable(&channel, 50, reuse),
            "a full window of 20 cannot answer a read of 50"
        );
        tokio::time::sleep(Duration::from_secs(16)).await;
        assert!(!windows.reusable(&channel, 20, reuse));
    }
}
