//! The owner's say over the backfill, per channel: its own history mode, and "read this one
//! next". `#602 history-settings-and-limits`.
//!
//! # What lives here
//!
//! * **Each channel's own history mode** (`recent` or `full`), chosen in Settings › Channels and
//!   kept in the durable store ([`crate::store::StateStore::history_modes`]). A channel with none
//!   follows `[message_cache] backfill`. The backfill reads the mode here on every step, so this
//!   is the in-memory copy the store's rows are loaded into once
//!   ([`HistoryControl::load`]) and kept in step with by the route that changes them.
//! * **The priority queue**: channels the owner asked to have read next
//!   ([`HistoryControl::prioritize`]), front first. The backfill's full walk takes the front one
//!   before any other channel of its provider, at that provider's backfill ceiling rather than
//!   the full walk's gentler `full_reads_per_minute`, and a channel leaves the queue when its walk
//!   reaches the beginning. It is how search's "Search further back" asks for more history. In
//!   memory only: a restart forgets it, and the full walk goes on in its usual order.
//! * **What each provider's pace stands at** ([`PaceReport`]): the backfill writes it, the routes
//!   and the diagnostics read it, so Settings can say why a walk is waiting.
//!
//! None of it is reachable from an MCP tool, for the `#39` reason the other owner settings give:
//! what this server reads of the owner's channels, and how hard, is his to decide.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

use crate::model::ChannelId;
use crate::store::{HistoryMode, HistoryModeSetting};

/// Why the backfill's reads of one provider are holding off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    /// The provider answered a backfill read with a rate limit the client could not wait out.
    RateLimited,
    /// A backfill read failed in a way that may pass (a 5xx, a timeout, a lost connection).
    UpstreamError,
    /// The provider signalled it to every request: a rate limit answered to some other read, or
    /// a count of rejected requests nearing its ban ([`crate::chat::ChatClient::crawl_hold`]).
    ProviderSignal,
    /// The cache file is above the share of `max_megabytes` at which the full walk waits.
    Storage,
}

/// Where one provider's backfill pace stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PaceReport {
    /// `[[providers]] backfill_reads_per_minute`.
    pub reads_per_minute: u32,
    /// Backfill reads of it in the last minute.
    pub reads_last_minute: u32,
    /// Until when, on the server's clock in milliseconds, its reads hold off; `None` when they
    /// do not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_until_ms: Option<i64>,
    /// Why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_because: Option<HoldReason>,
    /// Backfill reads in a row that failed in a way that may pass: the exponent of the backoff.
    pub failures: u32,
}

/// One channel the owner asked to have read next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Priority {
    /// The channel.
    pub channel: ChannelId,
    /// When it was asked for, on the server's clock in milliseconds.
    pub asked_ms: i64,
}

#[derive(Debug, Default)]
struct Inner {
    loaded: bool,
    modes: BTreeMap<ChannelId, HistoryMode>,
    queue: VecDeque<Priority>,
    paces: BTreeMap<String, PaceReport>,
}

/// See the module documentation.
#[derive(Debug, Default)]
pub struct HistoryControl {
    inner: Mutex<Inner>,
    wake: tokio::sync::Notify,
}

impl HistoryControl {
    /// No channel has a mode of its own, none is queued, and nothing is loaded yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Take the store's rows as every channel's own mode, replacing what was held.
    pub fn load(&self, settings: Vec<HistoryModeSetting>) {
        let mut inner = self.lock();
        inner.modes = settings
            .into_iter()
            .map(|setting| (setting.channel, setting.mode))
            .collect();
        inner.loaded = true;
    }

    /// Whether the store's rows have been loaded.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.lock().loaded
    }

    /// `channel`'s own mode, when the owner chose one.
    #[must_use]
    pub fn mode(&self, channel: &ChannelId) -> Option<HistoryMode> {
        self.lock().modes.get(channel).copied()
    }

    /// Record `channel`'s own mode, or with `None` that it follows the server's setting. The store
    /// is written first, by the caller.
    pub fn set(&self, channel: &ChannelId, mode: Option<HistoryMode>) {
        let mut inner = self.lock();
        match mode {
            Some(mode) => {
                inner.modes.insert(channel.clone(), mode);
            }
            None => {
                inner.modes.remove(channel);
            }
        }
        drop(inner);
        self.wake.notify_one();
    }

    /// Put `channel` at the front of the queue (moving it there when it is already queued, and
    /// keeping when it was first asked for), and wake the backfill. Returns its place: 0.
    pub fn prioritize(&self, channel: &ChannelId, now_ms: i64) -> usize {
        let mut inner = self.lock();
        let asked_ms = inner
            .queue
            .iter()
            .position(|entry| &entry.channel == channel)
            .and_then(|at| inner.queue.remove(at))
            .map_or(now_ms, |entry| entry.asked_ms);
        inner.queue.push_front(Priority {
            channel: channel.clone(),
            asked_ms,
        });
        drop(inner);
        self.wake.notify_one();
        0
    }

    /// Take `channel` off the queue. Returns whether it was on it.
    pub fn cancel(&self, channel: &ChannelId) -> bool {
        let mut inner = self.lock();
        let before = inner.queue.len();
        inner.queue.retain(|entry| &entry.channel != channel);
        before != inner.queue.len()
    }

    /// The queue, front first.
    #[must_use]
    pub fn queue(&self) -> Vec<Priority> {
        self.lock().queue.iter().cloned().collect()
    }

    /// `channel`'s place in the queue, 0 at the front.
    #[must_use]
    pub fn position(&self, channel: &ChannelId) -> Option<usize> {
        self.lock()
            .queue
            .iter()
            .position(|entry| &entry.channel == channel)
    }

    /// Keep only the queued channels `keep` admits: a channel no longer listed leaves the queue.
    pub fn retain(&self, keep: impl Fn(&ChannelId) -> bool) {
        self.lock().queue.retain(|entry| keep(&entry.channel));
    }

    /// Record where `provider`'s pace stands.
    pub fn set_pace(&self, provider: &str, report: PaceReport) {
        self.lock().paces.insert(provider.to_owned(), report);
    }

    /// Where `provider`'s pace stands, once the backfill has stepped.
    #[must_use]
    pub fn pace(&self, provider: &str) -> Option<PaceReport> {
        self.lock().paces.get(provider).cloned()
    }

    /// Every provider's pace, by key.
    #[must_use]
    pub fn paces(&self) -> BTreeMap<String, PaceReport> {
        self.lock().paces.clone()
    }

    /// Wait `pause`, or less when a channel is prioritized or its mode changes meanwhile.
    pub async fn pause(&self, pause: Duration) {
        let _ = tokio::time::timeout(pause, self.wake.notified()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: &str) -> ChannelId {
        ChannelId(id.to_owned())
    }

    #[test]
    fn a_bump_goes_to_the_front_and_keeps_when_it_was_first_asked() {
        let control = HistoryControl::new();
        control.prioritize(&ch("a"), 10);
        control.prioritize(&ch("b"), 20);
        assert_eq!(control.position(&ch("b")), Some(0));
        assert_eq!(control.position(&ch("a")), Some(1));
        control.prioritize(&ch("a"), 30);
        let queue = control.queue();
        assert_eq!(
            queue[0],
            Priority {
                channel: ch("a"),
                asked_ms: 10
            }
        );
        assert_eq!(queue[1].channel, ch("b"));
        assert!(control.cancel(&ch("a")));
        assert!(!control.cancel(&ch("a")));
        control.retain(|channel| channel != &ch("b"));
        assert!(control.queue().is_empty());
    }

    #[test]
    fn modes_load_from_the_store_and_follow_changes() {
        let control = HistoryControl::new();
        assert!(!control.is_loaded());
        control.load(vec![HistoryModeSetting {
            channel: ch("a"),
            mode: HistoryMode::Recent,
            updated_ms: 1,
        }]);
        assert!(control.is_loaded());
        assert_eq!(control.mode(&ch("a")), Some(HistoryMode::Recent));
        control.set(&ch("b"), Some(HistoryMode::Full));
        control.set(&ch("a"), None);
        assert_eq!(control.mode(&ch("a")), None);
        assert_eq!(control.mode(&ch("b")), Some(HistoryMode::Full));
    }
}
