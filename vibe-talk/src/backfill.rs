//! The backfill: the server's own reads that fill the message cache with history and keep what
//! it holds checked (`#453 cache-backfill`, `#595 cache-full-history`).
//!
//! # Why the cache is read again at all
//!
//! A provider's event stream carries new messages. It does not carry edits, deletions or other
//! bots' posts, and while an adapter is reconnecting (or the server is down) it carries nothing.
//! The relist ([`crate::relist`]) reads each channel's newest page every `[ingest]
//! relist_seconds`, which catches what changed at the newest end. What changed further back (an
//! edit of yesterday's message, a deletion last week, a post the stream missed while it was away)
//! only a walk back through the older pages finds. And the cache must find it: a held row that no
//! edit-and-delete-aware read has seen again within a week is erased
//! ([`crate::cache::REVALIDATE_WITHIN_MS`]), so that a message deleted upstream never outlives the
//! deletion here by more than a week.
//!
//! # The walks
//!
//! `[message_cache] backfill` is chosen when the server is set up:
//!
//! * **`recent`** (the default; `adaptive` is its first name) runs two walks of each cached
//!   channel, each from its newest page back through older ones, offered to the cache as one
//!   listing as of when it began, until the provider says nothing is older, the pages reach the
//!   walk's floor, or [`BACKFILL_PAGES`] pages have been read:
//!   * the **hot walk** re-reads the last `rewalk_hot_hours` (24) every `rewalk_hot_minutes` (60),
//!     and at once when a stream lost records or came back after a silence
//!     ([`crate::live::LiveHub::request_rewalk`]);
//!   * the **recent walk** re-reads the last `recent_days` (by default `max_days`, as far back as
//!     the walk it replaced went; 14 under `full`) every `rewalk_recent_hours` (24), and when the
//!     server starts, which covers whatever happened while it was down.
//! * **`full`** runs both, and a third, the **full walk**, which reads each channel's whole history
//!   into the cache: from the newest page back to the channel's beginning, one step back at a time,
//!   each page offered as a listing of the stretch between its oldest message and the step before
//!   it ([`crate::cache::observe::older`]), and each thread found on a page read whole before the
//!   walk steps back again. Where it stands is kept in the cache file
//!   ([`crate::cache::Frontier`]), so a restart resumes it where it stopped, as long as the cache
//!   still holds the stretch it stopped at; otherwise the walk starts again from the newest page.
//!   Every held row must be read again within a week, so the full walk does not stop at the
//!   beginning: `full_cycle_hours` (72) after one walk of a channel began, the next begins.
//!   There is no age limit unless `max_days` sets one; `max_megabytes` bounds the file, and the
//!   full walk pauses while the file is above [`FULL_STORAGE_PERCENT`] of it, so the cache's own
//!   eviction (which starts at the bound) does not take back what the walk just read.
//! * **`off`** makes none of these reads.
//!
//! A channel is complete back to its beginning once a full walk has reached it, and stays so while
//! the latest walk to reach it began less than a week ago: everything read since is newer than
//! that walk's start, everything older was read by it ([`history`]).
//!
//! # Pace
//!
//! Every backfill read waits until its provider has had nothing else running for
//! [`BACKFILL_IDLE`] ([`crate::upstream_profile::ProviderProfile::idle_for`]): a page reader, the
//! poller and a voice call always go first. The hot and recent walks read a page at least
//! [`BACKFILL_SPACING`] apart; the full walk reads at most `full_reads_per_minute` (6) pages a
//! minute of each provider, its threads' pages included, which stays well inside Slack's tier 3
//! (about 50 calls a minute for `conversations.history` and `conversations.replies`) and Discord's
//! per-route buckets for reading a channel's messages. Its reads ask for [`FULL_LIMIT`] rows. All
//! of them are counted under the read reason `backfill`.
//!
//! # Each channel's own mode, and the queue (`#602 history-settings-and-limits`)
//!
//! A channel may have a history mode of its own, chosen in Settings › Channels and kept in the
//! store ([`crate::history_control`]): `recent` takes it out of the full walk on a server set to
//! `full`, `full` puts it into the full walk on a server set to `recent`. `off` stays the
//! operator's switch for every read here. A channel the owner asked to have read next (search's
//! "Search further back") is walked before any other channel of its provider, whatever its mode,
//! at the provider's ceiling below rather than `full_reads_per_minute`, until its walk reaches the
//! beginning; one whose walk reached it less than a week ago has nothing further back, and leaves
//! the queue at once.
//!
//! # Rate limits: a ceiling of our own, and every signal the provider sends
//!
//! Every backfill read of one provider, its walks together, counts against
//! `[[providers]] backfill_reads_per_minute` over a sliding minute (20 for Slack, 30 for a
//! Discord-protocol provider: well under what each publishes; the README's rate-limit section
//! cites the policies). On top of that ceiling, the provider's own word wins:
//!
//! * **the client's own bounded waits** come first: a 429's `Retry-After` (Slack and Discord),
//!   Discord's `retry_after` body and its `X-RateLimit-Remaining`/`Reset-After` buckets are obeyed
//!   inside the read ([`crate::discord::ratelimit`], [`crate::slack`]);
//! * **a read that fails anyway** in a way that may pass (a rate limit the client could not wait
//!   out, a 5xx, a timeout) holds every backfill read of that provider for the longer of what the
//!   provider asked and an exponential backoff, from [`BACKOFF_FIRST`] doubling to [`BACKOFF_CAP`];
//!   the next read that succeeds resets it;
//! * **what the provider said to anyone** holds it too ([`crate::chat::ChatClient::crawl_hold`]):
//!   a rate limit answered to the page's own read, Discord's whole-token limit, or a count of
//!   401, 403 and 429 answers nearing Discord's invalid-request ban.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::Duration;

use serde::Serialize;

use crate::cache::observe::{Shape, TimelineRead};
use crate::cache::{ChanKey, Fill, FillGrant, Frontier, ServerFill, Stamp};
use crate::chat::ChatClient as _;
use crate::config::{CacheBackfill, MessageCacheConfig};
use crate::history_control::{HoldReason, PaceReport};
use crate::store::HistoryMode;
use crate::model::{ChannelId, Message};
use crate::read_reason::ReadReason;
use crate::state::AppState;
use crate::threads::{TimelinePage, TimelineRequest, TimelineView};

/// The most pages one hot or recent walk reads of one channel.
pub const BACKFILL_PAGES: usize = 20;
/// The least time between two backfill reads of the hot and recent walks.
pub const BACKFILL_SPACING: Duration = Duration::from_secs(1);
/// How long the provider must have had nothing else running before the backfill reads.
pub const BACKFILL_IDLE: Duration = Duration::from_secs(2);
/// How long a channel whose backfill read failed waits before it is read again.
pub const BACKFILL_RETRY: Duration = Duration::from_secs(10 * 60);
/// Rows each read of the full walk asks for; a provider may clamp it to its own page ceiling.
pub const FULL_LIMIT: u16 = 100;
/// The share of `max_megabytes`, in percent, above which the full walk waits.
pub const FULL_STORAGE_PERCENT: u64 = 85;
/// How many times one thread's read may fail before the full walk goes on without it.
pub const THREAD_ATTEMPTS: u8 = 3;
/// How many reads in a row resumed from where the full walk stopped may fail before it starts
/// again from the newest page.
pub const RESUME_ATTEMPTS: u8 = 2;
/// How long after a failed resumed read the full walk tries it again.
const RESUME_RETRY: Duration = Duration::from_secs(60);
/// The first wait after a backfill read of a provider failed in a way that may pass; it doubles
/// with each such failure in a row. `#602 history-settings-and-limits`.
pub const BACKOFF_FIRST: Duration = Duration::from_secs(30);
/// The longest wait [`BACKOFF_FIRST`] doubles to.
pub const BACKOFF_CAP: Duration = Duration::from_secs(30 * 60);
/// The window `[[providers]] backfill_reads_per_minute` is counted over.
const PACE_WINDOW_MS: i64 = 60 * 1000;
/// `below_ms` of a walk whose newest page held no message: nothing to step back below, so its
/// older pages are rows only.
const UNKNOWN_BELOW: i64 = i64::MIN;

const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Which walk from the newest page a channel is having.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The last `rewalk_hot_hours`.
    Hot,
    /// The last `recent_days`.
    Recent,
}

/// Where a hot or recent walk stands in one channel.
#[derive(Debug)]
struct Walk {
    tier: Tier,
    /// The continuation to read next; `None` before the newest page has been read.
    before: Option<String>,
    /// The fill begun before the walk's newest page was read, and every message the walk has read
    /// since, newest page first: offered together, once the walk ends, as one listing of the
    /// channel's own timeline from its oldest message to its newest end, as of when the walk
    /// began ([`crate::cache::Fill::offer_walk`]). Each page is offered as it is read too, but an
    /// older page alone is only rows: its span is proven by the walk it continues.
    listing: Option<Fill>,
    read: Vec<Message>,
    /// Whether any page of the walk came from a copy the provider could not bring up to date
    /// (`as_of`) or carried a notice: then the walk proves nothing, and only its rows are offered.
    unproven: bool,
    /// Pages read in this walk.
    pages: usize,
    /// Where the walk's listing may land, as the cache stamped it when the walk began: an erase of
    /// the channel, its provider or the file since then drops what the walk offers, so the walk,
    /// over or not, is walked again from its newest page. `#502 cache-serve-coverage`.
    stamp: Option<Stamp>,
    /// Provider time below which the walk has read far enough.
    floor_ms: i64,
}

/// One thread the full walk reads whole before it steps back again.
#[derive(Debug)]
struct ThreadJob {
    id: String,
    /// The continuation to read next, and the provider time of the oldest reply read so far;
    /// `None` before the thread's newest page has been read.
    next: Option<(String, i64)>,
    failures: u8,
}

/// A page of the full walk whose threads are still being read: where the walk will stand once
/// they are, recorded only then, so that a restart in between reads the page and its threads again.
#[derive(Debug)]
struct Pending {
    next: Frontier,
    /// Begun before the page was read, so the record lands under that read's stamp.
    fill: Option<Fill>,
    threads: VecDeque<ThreadJob>,
}

/// The full walk of one channel.
#[derive(Debug, Default)]
struct Cycle {
    /// Whether the record has been read from the file.
    loaded: bool,
    /// Where the walk stands; `None` before one began.
    frontier: Option<Frontier>,
    /// Start again from the newest page: the stretch the record stopped at is no longer held.
    restart: bool,
    /// The stamp of the latest read the record rests on: an erase since then starts afresh.
    stamp: Option<Stamp>,
    pending: Option<Pending>,
    /// Reads resumed from the record that failed in a row.
    resume_failures: u8,
}

impl Cycle {
    /// Whether a walk is part way: a page's threads pending, or older pages left.
    fn under_way(&self) -> bool {
        self.pending.is_some()
            || (!self.restart
                && self
                    .frontier
                    .as_ref()
                    .is_some_and(|frontier| frontier.cursor.is_some()))
    }
}

/// Everything the backfill knows of one channel.
#[derive(Debug, Default)]
struct Channel {
    walk: Option<Walk>,
    /// The stamp of the latest hot or recent walk that ended.
    walked: Option<Stamp>,
    /// Local times at which the next hot and recent walks are due; 0 is at once.
    hot_due_ms: i64,
    recent_due_ms: i64,
    /// A stream gap asked for a hot walk.
    rewalk: bool,
    /// Not before this local time, after a failure.
    retry_at_ms: Option<i64>,
    cycle: Cycle,
}

/// What one backfill step did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackfillStep {
    /// One page was read and offered.
    Read,
    /// One read failed; its channel waits [`BACKFILL_RETRY`].
    Failed,
    /// The provider of the next channel is busy.
    Busy,
    /// Every channel left to walk is of a provider whose account has not been checked against
    /// the cache file yet, so nothing read now would be written: their walks wait, from their
    /// newest pages.
    Unchecked,
    /// The full walk has pages left, and its provider's pace says not yet; or a provider's reads
    /// in the last minute are at its `backfill_reads_per_minute`.
    Waiting,
    /// Every channel with a read due is of a provider whose reads are holding off: after a
    /// failure, or because the provider said to. `#602 history-settings-and-limits`.
    Held,
    /// Nothing is due: every walk is over until its next interval, or waiting after a failure.
    Idle,
    /// The backfill is off, or there is no cache to fill.
    Off,
}

/// The job a channel has next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    Walk,
    Start(Tier),
    Full,
}

/// The `[message_cache]` table, when it asks for the backfill.
fn backfill_config(state: &AppState) -> Option<&MessageCacheConfig> {
    state
        .config
        .message_cache
        .as_ref()
        .filter(|config| config.backfill != CacheBackfill::Off)
}

/// The view the full walk reads of a channel: Flat for a bridge, whose Flat list is the
/// channel's whole list; Main otherwise, with each thread read on its own.
fn full_view(shape: Shape) -> TimelineView {
    if shape == Shape::Bridge {
        TimelineView::Flat
    } else {
        TimelineView::Main
    }
}

/// The threads with replies a page of the full walk shows: roots among its messages, and the
/// summaries it carries. A narrowed Slack channel and a bridge's Flat page have none to read apart.
fn threads_of(shape: Shape, page: &TimelinePage) -> VecDeque<ThreadJob> {
    if matches!(shape, Shape::SlackThread | Shape::Bridge) {
        return VecDeque::new();
    }
    let mut seen = BTreeSet::new();
    let from_messages = page
        .messages
        .iter()
        .filter_map(|message| message.thread.as_ref())
        .filter(|thread| thread.is_root && thread.reply_count != Some(0))
        .map(|thread| thread.id.clone());
    let from_summaries = page
        .threads
        .iter()
        .filter(|summary| summary.reply_count != Some(0))
        .map(|summary| summary.id.clone());
    from_messages
        .chain(from_summaries)
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .map(|id| ThreadJob {
            id,
            next: None,
            failures: 0,
        })
        .collect()
}

fn oldest_ms(messages: &[Message]) -> Option<i64> {
    messages
        .iter()
        .filter_map(|message| message.id.created_at_ms())
        .min()
}

/// The backfill's state across steps.
#[derive(Debug, Default)]
pub struct Backfill {
    channels: BTreeMap<ChannelId, Channel>,
    /// When each provider's full walk last read, local time: its pace.
    last_full_ms: HashMap<String, i64>,
    /// The channel each provider's full walk is reading: it stays there until that walk is over,
    /// so the provider's continuations are used while they are fresh (a Discord timeline cursor
    /// lives five minutes).
    focus: HashMap<String, ChannelId>,
    turn: usize,
    /// Each provider's pace: its backfill reads in the last minute, and any hold. `#602
    /// history-settings-and-limits`.
    pace: HashMap<String, ProviderPace>,
    /// Whether the step under way sent a read, and how it failed, when it did: what the
    /// provider's pace is told once the step is over.
    fetched: bool,
    failure: Option<Failure>,
}

/// One provider's backfill pace.
#[derive(Debug, Default)]
struct ProviderPace {
    /// When each backfill read inside the last [`PACE_WINDOW_MS`] was sent, oldest first.
    reads: VecDeque<i64>,
    /// No backfill read of it before this local time.
    hold_until_ms: i64,
    because: Option<HoldReason>,
    /// Reads in a row that failed in a way that may pass.
    failures: u32,
}

/// How a backfill read failed, for its provider's backoff.
#[derive(Clone, Copy, Debug)]
struct Failure {
    transient: bool,
    rate_limited: bool,
    retry_after: Option<Duration>,
}

impl Failure {
    fn of(error: &crate::chat::ChatError) -> Self {
        use crate::chat::ChatError;
        Self {
            transient: error.is_transient(),
            rate_limited: matches!(
                error.cause(),
                ChatError::RateLimited(_) | ChatError::Status { status: 429, .. }
            ),
            retry_after: error.retry_after(),
        }
    }
}

/// The wait after `failures` backfill reads of `provider` in a row failed in a way that may pass:
/// [`BACKOFF_FIRST`] doubling to [`BACKOFF_CAP`], plus up to a tenth more that depends only on the
/// provider, so two providers that failed together do not retry in the same instant.
#[must_use]
pub fn backoff(provider: &str, failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    let base = BACKOFF_FIRST
        .saturating_mul(1_u32 << doublings)
        .min(BACKOFF_CAP);
    let mut hash: u32 = 0x811c_9dc5;
    for byte in provider.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    base + base.mul_f64(f64::from(hash % 1000) / 10_000.0)
}

/// What a provider's pace allows now.
enum Gate {
    Open,
    /// Its reads in the last minute are at its ceiling.
    Paced,
    /// Its reads hold off.
    Held,
}

/// `[[providers]] backfill_reads_per_minute` of the provider `key`.
fn ceiling_of(state: &AppState, key: &str) -> u32 {
    state
        .config
        .providers
        .iter()
        .find(|provider| provider.key == key)
        .map_or(
            crate::config::DEFAULT_DISCORD_BACKFILL_READS_PER_MINUTE,
            |provider| provider.backfill_reads_per_minute,
        )
        .max(1)
}

/// Whether the latest full walk to reach `frontier`'s channel's beginning began less than a week
/// ago, so the cache holds all of it: as [`channel_history`] decides `complete`.
fn complete_at(frontier: &Frontier, now_ms: i64) -> bool {
    frontier
        .complete_started_ms
        .is_some_and(|began| now_ms.saturating_sub(began) < crate::cache::REVALIDATE_WITHIN_MS)
}

impl Backfill {
    /// A backfill that has walked nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Make every channel's recent walk due at once.
    pub fn rewalk(&mut self) {
        for channel in self.channels.values_mut() {
            channel.recent_due_ms = 0;
        }
    }

    /// What `key`'s pace allows at `now_ms`, with `signal` what its client says of crawling now.
    fn gate(&mut self, key: &str, ceiling: u32, signal: Option<Duration>, now_ms: i64) -> Gate {
        let pace = self.pace.entry(key.to_owned()).or_default();
        while pace
            .reads
            .front()
            .is_some_and(|&at| now_ms.saturating_sub(at) >= PACE_WINDOW_MS)
        {
            pace.reads.pop_front();
        }
        if let Some(wait) = signal {
            let until = now_ms.saturating_add(millis(wait));
            if until > pace.hold_until_ms {
                pace.hold_until_ms = until;
                pace.because = Some(HoldReason::ProviderSignal);
            }
        }
        if now_ms < pace.hold_until_ms {
            Gate::Held
        } else if pace.reads.len() >= usize::try_from(ceiling).unwrap_or(usize::MAX) {
            Gate::Paced
        } else {
            Gate::Open
        }
    }

    /// Tell `key`'s pace how the read just sent at `now_ms` came out.
    fn settle(&mut self, key: &str, now_ms: i64) {
        let failure = self.failure.take();
        if !std::mem::take(&mut self.fetched) {
            return;
        }
        let pace = self.pace.entry(key.to_owned()).or_default();
        pace.reads.push_back(now_ms);
        match failure {
            None => {
                pace.failures = 0;
            }
            Some(failure) if failure.transient => {
                pace.failures = pace.failures.saturating_add(1);
                let wait = backoff(key, pace.failures).max(failure.retry_after.unwrap_or_default());
                pace.hold_until_ms = now_ms.saturating_add(millis(wait));
                pace.because = Some(if failure.rate_limited {
                    HoldReason::RateLimited
                } else {
                    HoldReason::UpstreamError
                });
                tracing::info!(
                    provider = %key,
                    failures = pace.failures,
                    wait_s = wait.as_secs(),
                    rate_limited = failure.rate_limited,
                    "a backfill read failed in a way that may pass; this provider's backfill \
                     holds off"
                );
            }
            // A refusal of one channel's read (a 403, a channel gone) is that channel's: it waits
            // on its own, and the provider's other channels go on.
            Some(_) => {}
        }
    }

    /// Where each provider's pace stands, for the routes and the diagnostics.
    fn report(&self, state: &AppState, now_ms: i64) {
        let control = state.live.history();
        // Local (cache) time to the server's wall clock, for the page.
        let wall_offset = jiff::Timestamp::now().as_millisecond().saturating_sub(now_ms);
        for (key, pace) in &self.pace {
            let held = now_ms < pace.hold_until_ms;
            let reads = pace
                .reads
                .iter()
                .filter(|&&at| now_ms.saturating_sub(at) < PACE_WINDOW_MS)
                .count();
            control.set_pace(
                key,
                PaceReport {
                    reads_per_minute: ceiling_of(state, key),
                    reads_last_minute: u32::try_from(reads).unwrap_or(u32::MAX),
                    held_until_ms: held.then(|| pace.hold_until_ms.saturating_add(wall_offset)),
                    held_because: if held { pace.because } else { None },
                    failures: pace.failures,
                },
            );
        }
    }

    /// Read the next page some channel's walks have due, if its provider is idle.
    pub async fn step(&mut self, state: &AppState, limit: u16, idle: Duration) -> BackfillStep {
        let Some(config) = backfill_config(state) else {
            return BackfillStep::Off;
        };
        let Some(cache) = state.message_cache.cache().filter(|cache| cache.is_open()) else {
            return BackfillStep::Off;
        };
        let now_ms = cache.now_ms();
        let channels: Vec<(ChannelId, ChanKey)> = state
            .all_channels()
            .into_iter()
            .filter_map(|channel| {
                let chan = state.message_cache.chan_key(&channel.id)?;
                Some((channel.id, chan))
            })
            .collect();
        let listed: BTreeSet<&ChannelId> = channels.iter().map(|(id, _)| id).collect();
        let current = |chan: &ChanKey, stamp: Option<Stamp>| {
            stamp.is_none_or(|stamp| cache.stamp(chan) == stamp)
        };
        self.channels.retain(|channel, _| listed.contains(channel));
        for channel in state.live.take_rewalk_requests() {
            if let Some(entry) = self.channels.get_mut(&channel) {
                entry.rewalk = true;
            }
        }
        for (id, chan) in &channels {
            let entry = self.channels.entry(id.clone()).or_default();
            // A walk whose listing an erase has made stale since it began proves nothing however
            // it ends, nor one that is over: it walks again (`#502 cache-serve-coverage`).
            if !current(chan, entry.walk.as_ref().and_then(|walk| walk.stamp))
                || !current(chan, entry.walked)
            {
                entry.walk = None;
                entry.walked = None;
                entry.recent_due_ms = 0;
            }
            // So does the full walk, from what the file says once the erase has been applied.
            if !current(chan, entry.cycle.stamp) {
                entry.cycle = Cycle::default();
            }
        }
        // `#602 history-settings-and-limits`: each channel's own mode, and the channels the owner
        // asked to have read next, which go first, in the queue's order.
        let control = state.live.history();
        control.retain(|channel| listed.contains(channel));
        let queue: Vec<ChannelId> = control
            .queue()
            .into_iter()
            .map(|priority| priority.channel)
            .collect();
        let server_full = config.backfill == CacheBackfill::Full;
        let storage_ok = cache.bytes_on_disk().saturating_mul(100)
            < u64::from(config.max_megabytes) * 1024 * 1024 * FULL_STORAGE_PERCENT;
        let pace_ms = MINUTE_MS / i64::from(config.full_reads_per_minute.max(1));
        let paced = |key: Option<&String>, last: &HashMap<String, i64>| {
            key.and_then(|key| last.get(key))
                .is_some_and(|&at| now_ms.saturating_sub(at) < pace_ms)
        };
        // A channel whose provider's account has not been checked against the file is not walked
        // yet: nothing read would be written. It holds back no other provider's channels.
        let checked = |channel: &ChannelId| {
            state.providers.key_for(channel).is_some_and(|key| {
                state.message_cache.verification(&key) == crate::cache::sink::Verification::Verified
            })
        };
        let count = channels.len();
        let mut order: Vec<usize> = queue
            .iter()
            .filter_map(|queued| channels.iter().position(|(id, _)| id == queued))
            .collect();
        for offset in 0..count {
            let index = (self.turn + offset) % count.max(1);
            if !order.contains(&index) {
                order.push(index);
            }
        }
        let mut waiting_unchecked = false;
        let mut waiting_pace = false;
        let mut held = false;
        let mut finished: Vec<ChannelId> = Vec::new();
        let mut chosen: Option<(ChannelId, ChanKey, Job)> = None;
        for index in order {
            let (id, chan) = &channels[index];
            let key = state.providers.key_for(id);
            let prioritized = queue.contains(id);
            let Some(entry) = self.channels.get_mut(id) else {
                continue;
            };
            if entry.retry_at_ms.is_some_and(|at| now_ms < at) {
                continue;
            }
            let full = prioritized
                || control.mode(id).map_or(server_full, |mode| mode == HistoryMode::Full);
            let job = if entry.walk.is_some() {
                Some(Job::Walk)
            } else if now_ms >= entry.recent_due_ms {
                Some(Job::Start(Tier::Recent))
            } else if entry.rewalk || now_ms >= entry.hot_due_ms {
                Some(Job::Start(Tier::Hot))
            } else if full && storage_ok && {
                let due = self::full_due(state, &cache, chan, entry, config, now_ms);
                if !due && prioritized && entry.cycle.loaded {
                    // Asked for, and between walks: a walk that reached the beginning less than a
                    // week ago left nothing further back to read; otherwise a walk begins now.
                    if entry
                        .cycle
                        .frontier
                        .as_ref()
                        .is_some_and(|frontier| complete_at(frontier, now_ms))
                    {
                        finished.push(id.clone());
                        false
                    } else {
                        entry.cycle.restart = true;
                        true
                    }
                } else {
                    due
                }
            } {
                // A prioritized channel keeps to its provider's ceiling alone.
                if !prioritized && paced(key.as_ref(), &self.last_full_ms) {
                    waiting_pace = true;
                    None
                } else {
                    Some(Job::Full)
                }
            } else {
                None
            };
            let Some(job) = job else {
                continue;
            };
            if !checked(id) {
                waiting_unchecked = true;
                continue;
            }
            if let Some(key) = &key {
                let signal = state
                    .providers
                    .client_for(id)
                    .and_then(|client| client.crawl_hold());
                match self.gate(key, ceiling_of(state, key), signal, now_ms) {
                    Gate::Open => {}
                    Gate::Paced => {
                        waiting_pace = true;
                        continue;
                    }
                    Gate::Held => {
                        held = true;
                        continue;
                    }
                }
            }
            chosen = Some((id.clone(), chan.clone(), job));
            break;
        }
        for channel in &finished {
            control.cancel(channel);
        }
        // The full walk stays on the channel its provider is reading until that walk is over,
        // unless the owner asked for this one.
        if let Some((id, _, Job::Full)) = &chosen {
            if let Some(key) = state.providers.key_for(id) {
                let focused = self
                    .focus
                    .get(&key)
                    .filter(|focus| *focus != id && !queue.contains(id))
                    .and_then(|focus| {
                        let entry = self.channels.get(focus)?;
                        let free = entry.walk.is_none()
                            && entry.retry_at_ms.is_none_or(|at| now_ms >= at)
                            && entry.cycle.under_way();
                        let chan = channels.iter().find(|(c, _)| c == focus)?.1.clone();
                        free.then(|| (focus.clone(), chan, Job::Full))
                    });
                if let Some(focused) = focused {
                    chosen = Some(focused);
                }
                if let Some((id, _, _)) = &chosen {
                    self.focus.insert(key, id.clone());
                }
            }
        }
        let Some((channel, _, job)) = chosen else {
            self.report(state, now_ms);
            return if waiting_unchecked {
                BackfillStep::Unchecked
            } else if held {
                BackfillStep::Held
            } else if waiting_pace {
                BackfillStep::Waiting
            } else {
                BackfillStep::Idle
            };
        };
        let key = state.providers.key_for(&channel);
        let busy = key
            .as_ref()
            .and_then(|key| state.providers.profile().find(key))
            .is_some_and(|profile| profile.idle_for().is_none_or(|for_| for_ < idle));
        // The turn moves on either way, so one provider that is never idle does not hold back
        // every other provider's channels.
        self.turn = self.turn.wrapping_add(1);
        if busy {
            return BackfillStep::Busy;
        }
        self.fetched = false;
        self.failure = None;
        let step = match job {
            Job::Walk | Job::Start(_) => {
                let tier = match job {
                    Job::Start(tier) => Some(tier),
                    _ => None,
                };
                self.walk_step(state, &channel, config, tier, limit, now_ms)
                    .await
            }
            Job::Full => {
                if let Some(key) = &key {
                    self.last_full_ms.insert(key.clone(), now_ms);
                }
                let step = self.full_step(state, &channel, config, now_ms).await;
                // A walk asked for is over once it has reached the beginning.
                let reached = self.channels.get(&channel).is_some_and(|entry| {
                    entry.cycle.pending.is_none()
                        && entry
                            .cycle
                            .frontier
                            .as_ref()
                            .is_some_and(|frontier| frontier.cursor.is_none())
                });
                if reached && control.cancel(&channel) {
                    tracing::info!(channel = %channel, "the walk asked for reached the channel's beginning");
                }
                step
            }
        };
        if let Some(key) = &key {
            self.settle(key, now_ms);
        }
        self.report(state, now_ms);
        step
    }

    /// One page of a hot or recent walk: the next of the walk under way, or the newest page of
    /// the walk `start` begins.
    async fn walk_step(
        &mut self,
        state: &AppState,
        channel: &ChannelId,
        config: &MessageCacheConfig,
        start: Option<Tier>,
        limit: u16,
        now_ms: i64,
    ) -> BackfillStep {
        let Some(entry) = self.channels.get_mut(channel) else {
            return BackfillStep::Idle;
        };
        let before = entry.walk.as_ref().and_then(|walk| walk.before.clone());
        let fill = state
            .message_cache
            .begin(&FillGrant::server(ServerFill::Backfill), channel);
        let listing = start.map(|_| {
            state
                .message_cache
                .begin(&FillGrant::server(ServerFill::Backfill), channel)
        });
        // `#502 cache-serve-coverage`. A fill begun before the provider's account was checked
        // against the file writes no rows, and its offer leaves the channel in doubt: a page read
        // then is spent for nothing, and a walk whose listing began then proves nothing however
        // it ends. So the walk waits for the check, and one that loses it (a suspension, an erase
        // of the file) starts over from its newest page.
        if fill
            .iter()
            .chain(listing.iter().flatten())
            .any(|fill| !fill.writes_rows())
        {
            entry.walk = None;
            return BackfillStep::Unchecked;
        }
        if let Some(tier) = start {
            let cutoff =
                crate::cache::Limits::from_config(config).cutoff_ms(channel.as_str(), now_ms);
            let window_ms = match tier {
                Tier::Hot => i64::from(config.rewalk_hot_hours) * HOUR_MS,
                Tier::Recent => i64::from(config.recent_days) * DAY_MS,
            };
            let listing = listing.flatten();
            entry.walk = Some(Walk {
                tier,
                before: None,
                stamp: listing.as_ref().map(Fill::stamp),
                listing,
                read: Vec::new(),
                unproven: false,
                pages: 0,
                floor_ms: now_ms.saturating_sub(window_ms).max(cutoff),
            });
            // The recent walk reads everything the hot walk would.
            let hot_next = now_ms.saturating_add(i64::from(config.rewalk_hot_minutes) * MINUTE_MS);
            entry.hot_due_ms = hot_next;
            entry.rewalk = false;
            if tier == Tier::Recent {
                entry.recent_due_ms =
                    now_ms.saturating_add(i64::from(config.rewalk_recent_hours) * HOUR_MS);
            }
        }
        let Some(walk) = entry.walk.as_mut() else {
            return BackfillStep::Idle;
        };
        let request = TimelineRequest {
            view: TimelineView::Main,
            before,
            limit,
            ..TimelineRequest::default()
        };
        let read = if walk.before.is_some() {
            TimelineRead::Before
        } else {
            TimelineRead::Newest
        };
        self.fetched = true;
        let answer = ReadReason::Backfill
            .scope(state.providers.fetch_timeline(channel, &request))
            .await;
        let page = match answer {
            Ok(page) => page,
            Err(error) => {
                self.failure = Some(Failure::of(&error));
                tracing::debug!(channel = %channel, %error, "a backfill read failed; it is tried again later");
                entry.retry_at_ms = Some(now_ms.saturating_add(millis(BACKFILL_RETRY)));
                return BackfillStep::Failed;
            }
        };
        if let Some(fill) = fill {
            fill.offer_timeline(&request, read, &page);
        }
        walk.unproven |= page.as_of.is_some() || page.notice.is_some();
        walk.read.extend(page.messages.iter().cloned());
        walk.pages += 1;
        entry.retry_at_ms = None;
        let below_floor = oldest_ms(&page.messages).is_some_and(|oldest| oldest < walk.floor_ms);
        walk.before = page.next_before.clone();
        let done =
            !page.has_more || walk.before.is_none() || below_floor || walk.pages >= BACKFILL_PAGES;
        if done {
            let read = std::mem::take(&mut walk.read);
            if let Some(listing) = walk.listing.take() {
                if !walk.unproven {
                    listing.offer_walk(&read);
                }
            }
            entry.walked = walk.stamp;
            tracing::debug!(channel = %channel, tier = ?walk.tier, pages = walk.pages, "a backfill walk ended");
            entry.walk = None;
        }
        BackfillStep::Read
    }

    /// One read of the full walk of `channel`: the next page of a thread found on the page it
    /// stands at, or the next page back, or the newest page of a walk that begins now.
    async fn full_step(
        &mut self,
        state: &AppState,
        channel: &ChannelId,
        config: &MessageCacheConfig,
        now_ms: i64,
    ) -> BackfillStep {
        let Some(entry) = self.channels.get_mut(channel) else {
            return BackfillStep::Idle;
        };
        let fill = state
            .message_cache
            .begin(&FillGrant::server(ServerFill::Backfill), channel);
        let Some(fill) = fill else {
            return BackfillStep::Off;
        };
        if !fill.writes_rows() {
            entry.cycle.pending = None;
            return BackfillStep::Unchecked;
        }
        let shape = fill.shape();
        if let Some(pending) = entry.cycle.pending.as_mut() {
            let Some(mut job) = pending.threads.pop_front() else {
                finish_pending(&mut entry.cycle, channel);
                return BackfillStep::Idle;
            };
            let request = TimelineRequest {
                view: TimelineView::Thread,
                thread_id: Some(job.id.clone()),
                before: job.next.as_ref().map(|(cursor, _)| cursor.clone()),
                limit: FULL_LIMIT,
                ..TimelineRequest::default()
            };
            self.fetched = true;
            let answer = ReadReason::Backfill
                .scope(state.providers.fetch_timeline(channel, &request))
                .await;
            let page = match answer {
                Ok(page) => page,
                Err(error) => {
                    self.failure = Some(Failure::of(&error));
                    job.failures += 1;
                    tracing::debug!(channel = %channel, %error, failures = job.failures, "a full-history thread read failed");
                    give_up_or_requeue(pending, job);
                    entry.retry_at_ms = Some(now_ms.saturating_add(millis(BACKFILL_RETRY)));
                    return BackfillStep::Failed;
                }
            };
            let (queued, proven) = match job.next.as_ref() {
                Some(&(_, below_ms)) => (
                    fill.offer_older(&request, &page, below_ms),
                    proves_below(&page, below_ms),
                ),
                None => (
                    fill.offer_timeline(&request, TimelineRead::Newest, &page),
                    page.as_of.is_none() && page.notice.is_none(),
                ),
            };
            if !queued {
                // The cache had no room for it: read it again, as after a failure.
                job.failures += 1;
                give_up_or_requeue(pending, job);
                return BackfillStep::Read;
            }
            if !proven {
                pending.next.cycle_gaps += 1;
            }
            let below = oldest_ms(&page.messages).or(job.next.as_ref().map(|&(_, below)| below));
            match (page.next_before.clone().filter(|_| page.has_more), below) {
                // A continuation that does not move would read the same page for ever.
                (Some(next), _) if request.before.as_deref() == Some(next.as_str()) => {
                    tracing::warn!(channel = %channel, "a thread's continuation did not move; the full walk goes on without the rest of it");
                    pending.next.cycle_gaps += 1;
                }
                (Some(next), Some(below)) => {
                    job.next = Some((next, below));
                    pending.threads.push_front(job);
                }
                // More, but no message to step back below: nothing older can be proven.
                (Some(_), None) => pending.next.cycle_gaps += 1,
                (None, _) => {}
            }
            if pending.threads.is_empty() {
                finish_pending(&mut entry.cycle, channel);
            }
            return BackfillStep::Read;
        }
        // Where the walk reads next: on from the record, or the newest page of a new walk.
        let resume = entry
            .cycle
            .frontier
            .as_ref()
            .filter(|_| !entry.cycle.restart)
            .and_then(|frontier| {
                frontier
                    .cursor
                    .clone()
                    .map(|cursor| (cursor, frontier.below_ms))
            });
        let record = state
            .message_cache
            .begin(&FillGrant::server(ServerFill::Backfill), channel)
            .filter(Fill::writes_rows);
        let request = TimelineRequest {
            view: full_view(shape),
            before: resume.as_ref().map(|(cursor, _)| cursor.clone()),
            limit: FULL_LIMIT,
            ..TimelineRequest::default()
        };
        let stamp = fill.stamp();
        self.fetched = true;
        let answer = ReadReason::Backfill
            .scope(state.providers.fetch_timeline(channel, &request))
            .await;
        if let Err(error) = &answer {
            self.failure = Some(Failure::of(error));
        }
        let page = match answer {
            Ok(page) => page,
            Err(error) if resume.is_some() => {
                // A continuation may not outlive the provider's memory of it (a snapshot that
                // expires, a restart, a view that changed): after a second failure in a row the
                // walk starts again from the newest page rather than asking for ever.
                entry.cycle.resume_failures += 1;
                if entry.cycle.resume_failures >= RESUME_ATTEMPTS {
                    tracing::info!(channel = %channel, %error, "the full-history walk could not go on from where it stopped; it starts again from the newest page");
                    entry.cycle.restart = true;
                    entry.cycle.resume_failures = 0;
                } else {
                    tracing::debug!(channel = %channel, %error, "a full-history read failed; it is tried again shortly");
                    entry.retry_at_ms = Some(now_ms.saturating_add(millis(RESUME_RETRY)));
                }
                return BackfillStep::Failed;
            }
            Err(error) => {
                tracing::debug!(channel = %channel, %error, "a full-history read failed; it is tried again later");
                entry.retry_at_ms = Some(now_ms.saturating_add(millis(BACKFILL_RETRY)));
                return BackfillStep::Failed;
            }
        };
        entry.retry_at_ms = None;
        entry.cycle.resume_failures = 0;
        let (queued, proven) = match &resume {
            // A walk whose newest page held no message has nothing to step back below: its
            // older pages are rows only.
            Some((_, below_ms)) if *below_ms == UNKNOWN_BELOW => (
                fill.offer_timeline(&request, TimelineRead::Before, &page),
                page.messages.is_empty(),
            ),
            Some((_, below_ms)) => (
                fill.offer_older(&request, &page, *below_ms),
                proves_below(&page, *below_ms),
            ),
            None => (
                fill.offer_timeline(&request, TimelineRead::Newest, &page),
                page.as_of.is_none() && page.notice.is_none(),
            ),
        };
        if !queued {
            // The cache had no room for the page: the record stays where it was, and the page is
            // read again at the walk's next turn.
            tracing::debug!(channel = %channel, "a full-history page was not queued; it is read again");
            return BackfillStep::Read;
        }
        let previous = entry.cycle.frontier.clone();
        let mut gaps = match (&resume, &previous) {
            (Some(_), Some(previous)) => previous.cycle_gaps,
            _ => 0,
        };
        if !proven {
            gaps += 1;
        }
        let mut cursor = page.next_before.clone().filter(|_| page.has_more);
        if cursor.is_some() && cursor == request.before {
            tracing::warn!(channel = %channel, "the channel's continuation did not move; this full-history walk ends here, incomplete");
            gaps += 1;
            cursor = None;
        }
        let (cycle_started_ms, cycle_pages, below) = match (&resume, &previous) {
            (Some((_, below)), Some(previous)) => (
                previous.cycle_started_ms,
                previous.cycle_pages.saturating_add(1),
                oldest_ms(&page.messages).unwrap_or(*below),
            ),
            _ => (
                now_ms,
                1,
                oldest_ms(&page.messages).unwrap_or(UNKNOWN_BELOW),
            ),
        };
        // A channel kept for a limited time is complete once the walk is past its age limit.
        let cutoff = crate::cache::Limits::from_config(config).cutoff_ms(channel.as_str(), now_ms);
        let cursor = cursor.filter(|_| below == UNKNOWN_BELOW || below >= cutoff);
        let next = Frontier {
            cursor,
            below_ms: below,
            cycle_started_ms,
            cycle_pages,
            cycle_gaps: gaps,
            complete_ms: previous.as_ref().and_then(|p| p.complete_ms),
            complete_started_ms: previous.as_ref().and_then(|p| p.complete_started_ms),
            complete_pages: previous.as_ref().and_then(|p| p.complete_pages),
            updated_ms: now_ms,
        };
        let threads = if request.view == TimelineView::Main {
            threads_of(shape, &page)
        } else {
            VecDeque::new()
        };
        entry.cycle.restart = false;
        entry.cycle.stamp = Some(stamp);
        entry.cycle.pending = Some(Pending {
            next,
            fill: record,
            threads,
        });
        if entry
            .cycle
            .pending
            .as_ref()
            .is_some_and(|pending| pending.threads.is_empty())
        {
            finish_pending(&mut entry.cycle, channel);
        }
        BackfillStep::Read
    }
}

/// Whether a step back from below `below_ms` proves its stretch, as [`crate::cache::observe::older`]
/// decides: from an up-to-date copy, with no notice, and below the step before.
fn proves_below(page: &TimelinePage, below_ms: i64) -> bool {
    page.as_of.is_none()
        && page.notice.is_none()
        && oldest_and_newest(&page.messages).is_none_or(|(_, newest)| newest < below_ms)
}

fn oldest_and_newest(messages: &[Message]) -> Option<(i64, i64)> {
    let times = messages
        .iter()
        .filter_map(|message| message.id.created_at_ms());
    times.fold(None, |span, at| match span {
        None => Some((at, at)),
        Some((oldest, newest)) => Some((oldest.min(at), newest.max(at))),
    })
}

/// Put a thread job back to be read again, or give it up after [`THREAD_ATTEMPTS`] failures,
/// counting a gap in the walk.
fn give_up_or_requeue(pending: &mut Pending, job: ThreadJob) {
    if job.failures < THREAD_ATTEMPTS {
        pending.threads.push_front(job);
    } else {
        pending.next.cycle_gaps += 1;
    }
}

/// Record where the walk stands, now that the page it read and that page's threads are offered.
/// A walk that has reached the beginning makes the channel complete only if every read on the way
/// proved its stretch.
fn finish_pending(cycle: &mut Cycle, channel: &ChannelId) {
    if let Some(mut pending) = cycle.pending.take() {
        let next = &mut pending.next;
        if next.cursor.is_none() {
            let hours = (next.updated_ms - next.cycle_started_ms) / HOUR_MS;
            if next.cycle_gaps == 0 {
                next.complete_ms = Some(next.updated_ms);
                next.complete_started_ms = Some(next.cycle_started_ms);
                next.complete_pages = Some(next.cycle_pages);
                tracing::info!(channel = %channel, pages = next.cycle_pages, hours, "the full-history walk reached the channel's beginning");
            } else {
                tracing::warn!(channel = %channel, pages = next.cycle_pages, gaps = next.cycle_gaps, hours, "the full-history walk reached the channel's beginning with reads that proved nothing; the channel is not complete until a walk has none");
            }
        }
        if let Some(fill) = pending.fill {
            fill.offer_frontier(pending.next.clone());
        }
        cycle.frontier = Some(pending.next);
    }
}

/// Whether the full walk of a channel has a read due: a thread of the page it stands at, a page
/// back, or a new walk because the last began `full_cycle_hours` ago. Loads the record the first
/// time, and checks a record that stopped part way against what the cache still holds: when the
/// stretch it stopped at is gone (an erase of the channel's coverage, the week's revalidation
/// after a long stop, eviction), the walk starts again from the newest page.
fn full_due(
    state: &AppState,
    cache: &crate::cache::MessageCache,
    chan: &ChanKey,
    entry: &mut Channel,
    config: &MessageCacheConfig,
    now_ms: i64,
) -> bool {
    let cycle = &mut entry.cycle;
    if !cycle.loaded {
        let Some(frontier) = cache.frontier(chan).served() else {
            return false;
        };
        cycle.loaded = true;
        cycle.stamp = Some(cache.stamp(chan));
        if let Some(frontier) = &frontier {
            if frontier.cursor.is_some() {
                let cutoff =
                    crate::cache::Limits::from_config(config).cutoff_ms(&chan.channel_id, now_ms);
                let point = frontier.below_ms.max(cutoff);
                let shape = state
                    .message_cache
                    .begin(
                        &FillGrant::server(ServerFill::Backfill),
                        &ChannelId(chan.channel_id.clone()),
                    )
                    .map(|fill| fill.shape());
                // The scope the walk's pages list: a bridge's Flat list, or Main.
                let scope = if shape == Some(Shape::Bridge) {
                    crate::cache::Scope::Flat
                } else {
                    crate::cache::Scope::Main
                };
                let held = cache
                    .coverage(chan, &scope)
                    .served()
                    .is_some_and(|intervals| {
                        intervals
                            .iter()
                            .any(|interval| interval.t0_ms <= point && point < interval.t1_ms)
                    });
                cycle.restart = !held;
                if !held {
                    tracing::info!(
                        channel = %chan.channel_id,
                        "the cache no longer holds where the full-history walk stopped; it starts \
                         again from the newest page"
                    );
                }
            }
        }
        cycle.frontier = frontier;
    }
    if cycle.pending.is_some() || cycle.restart {
        return true;
    }
    match &cycle.frontier {
        None => true,
        Some(frontier) if frontier.cursor.is_some() => true,
        Some(frontier) => {
            let began = frontier
                .complete_started_ms
                .unwrap_or(frontier.cycle_started_ms);
            now_ms >= began.saturating_add(i64::from(config.full_cycle_hours) * HOUR_MS)
        }
    }
}

/// Walk every cached channel's history into the message cache, forever, a page at a time while
/// providers are idle. Spawned by `main` when a cache is configured; returns at once when the
/// backfill is off.
pub async fn backfill_forever(state: AppState, limit: u16) {
    if backfill_config(&state).is_none() {
        return;
    }
    // Each channel's own mode, from the store, before the first read: a channel the owner took
    // out of the full walk is never walked into older history first. `#602
    // history-settings-and-limits`.
    let control = std::sync::Arc::clone(state.live.history());
    while !control.is_loaded() {
        match state.store.history_modes().await {
            Ok(settings) => control.load(settings),
            // No store: no channel has a mode of its own, and Settings offers none.
            Err(crate::store::StoreError::Unavailable(_)) => control.load(Vec::new()),
            Err(error) => {
                tracing::warn!(%error, "the channels' history modes could not be read; the backfill waits and tries again");
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    }
    let mut backfill = Backfill::new();
    loop {
        let pause = match backfill.step(&state, limit, BACKFILL_IDLE).await {
            BackfillStep::Read
            | BackfillStep::Failed
            | BackfillStep::Busy
            | BackfillStep::Unchecked
            | BackfillStep::Waiting
            | BackfillStep::Held => BACKFILL_SPACING,
            // The owner turned the cache off, or it is not open yet: look again in a while.
            BackfillStep::Off => Duration::from_secs(30),
            BackfillStep::Idle => Duration::from_secs(15),
        };
        // A channel prioritized meanwhile, or a mode changed, ends the pause early.
        control.pause(pause).await;
    }
}

/// How the backfill stands, for the diagnostics and the cache status. `#595 cache-full-history`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HistoryReport {
    /// `recent`, `full` or `off`.
    pub mode: &'static str,
    /// The walks' intervals and pace, as configured.
    pub recent_days: u16,
    /// See [`MessageCacheConfig::rewalk_hot_hours`].
    pub rewalk_hot_hours: u16,
    /// See [`MessageCacheConfig::rewalk_hot_minutes`].
    pub rewalk_hot_minutes: u32,
    /// See [`MessageCacheConfig::rewalk_recent_hours`].
    pub rewalk_recent_hours: u32,
    /// See [`MessageCacheConfig::full_reads_per_minute`].
    pub full_reads_per_minute: u32,
    /// See [`MessageCacheConfig::full_cycle_hours`].
    pub full_cycle_hours: u32,
    /// `storage` while the full walk waits for room under `max_megabytes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused: Option<&'static str>,
    /// The file's bytes, its working files included, over the messages it holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_per_message: Option<u64>,
    /// Where each provider's backfill pace stands, by key, once the backfill has stepped. `#602
    /// history-settings-and-limits`.
    pub providers: BTreeMap<String, PaceReport>,
    /// Each cached channel.
    pub channels: Vec<ChannelHistory>,
}

/// How one channel's history stands. Times are RFC 3339.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelHistory {
    /// The channel's id, and its label in the configuration.
    pub channel: String,
    /// See `channel`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How far back the backfill reads it: its own mode when the owner chose one, otherwise the
    /// server's. `#602 history-settings-and-limits`.
    pub mode: &'static str,
    /// The owner's own choice for it, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_override: Option<&'static str>,
    /// Its place among the channels asked to be read next, 0 at the front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<usize>,
    /// Messages the file holds of it, and the bytes of their text.
    pub messages: u64,
    /// See `messages`.
    pub content_bytes: u64,
    /// The oldest and newest message held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest: Option<String>,
    /// See `oldest`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest: Option<String>,
    /// Whether the cache holds every message back to the channel's beginning: a full walk
    /// reached it, and the latest to reach it began less than a week ago.
    pub complete: bool,
    /// When the latest walk to reach the beginning ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete_at: Option<String>,
    /// Whether a full walk is under way, and the oldest message it has reached.
    pub walking: bool,
    /// See `walking`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reached: Option<String>,
    /// Pages the walk under way has read, and its pace so far.
    pub cycle_pages: i64,
    /// Its reads that proved nothing or were given up: a walk with any does not make the channel
    /// complete.
    pub cycle_gaps: i64,
    /// See `cycle_pages`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages_per_hour: Option<f64>,
    /// Days of history the walk under way has gone back per hour of walking: the pace of a first
    /// walk, whose length nothing knows yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_per_hour: Option<f64>,
    /// Pages and hours the latest walk to reach the beginning took.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle_pages: Option<i64>,
    /// See `last_cycle_pages`. Above 168 the week's revalidation erases the oldest rows before
    /// the next walk reads them again: raise `full_reads_per_minute`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle_hours: Option<f64>,
    /// Hours until the walk under way reaches the beginning, from the latest walk's length in
    /// pages and this one's pace; absent before any walk has reached it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_hours: Option<f64>,
}

fn hours(ms: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let hours = ms as f64 / HOUR_MS as f64;
    hours
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// How one channel's history stands, from what the file holds and where its walk stands, at
/// `now_ms`.
#[must_use]
pub fn channel_history(
    stats: &crate::cache::ChannelStats,
    frontier: Option<&Frontier>,
    now_ms: i64,
) -> ChannelHistory {
    let iso = crate::clock::iso_from_ms;
    let complete = frontier
        .and_then(|f| f.complete_started_ms)
        .is_some_and(|began| now_ms.saturating_sub(began) < crate::cache::REVALIDATE_WITHIN_MS);
    let walking = frontier.is_some_and(|f| f.cursor.is_some());
    let elapsed = frontier.map_or(0, |f| f.updated_ms.saturating_sub(f.cycle_started_ms));
    #[allow(clippy::cast_precision_loss)]
    let pages_per_hour = frontier
        .filter(|f| f.cycle_pages > 1 && elapsed > 0)
        .map(|f| round1(f.cycle_pages as f64 / hours(elapsed)));
    let days_per_hour = frontier
        .filter(|f| f.cursor.is_some() && f.below_ms != UNKNOWN_BELOW && elapsed > 0)
        .map(|f| {
            round1(hours(f.cycle_started_ms.saturating_sub(f.below_ms)) / 24.0 / hours(elapsed))
        });
    #[allow(clippy::cast_precision_loss)]
    let eta_hours = frontier.filter(|f| f.cursor.is_some()).and_then(|f| {
        let total = f.complete_pages?;
        let rate = pages_per_hour?;
        (rate > 0.0).then(|| round1((total - f.cycle_pages).max(0) as f64 / rate))
    });
    ChannelHistory {
        channel: stats.chan.channel_id.clone(),
        label: None,
        mode: "recent",
        mode_override: None,
        queue_position: None,
        messages: stats.messages,
        content_bytes: stats.content_bytes,
        oldest: stats.oldest_ms.map(iso),
        newest: stats.newest_ms.map(iso),
        complete,
        complete_at: frontier.and_then(|f| f.complete_ms).map(iso),
        walking,
        reached: frontier
            .filter(|f| f.cursor.is_some() && f.below_ms != UNKNOWN_BELOW)
            .map(|f| iso(f.below_ms)),
        cycle_pages: frontier.map_or(0, |f| f.cycle_pages),
        cycle_gaps: frontier.map_or(0, |f| f.cycle_gaps),
        pages_per_hour,
        days_per_hour,
        last_cycle_pages: frontier.and_then(|f| f.complete_pages),
        last_cycle_hours: frontier.and_then(|f| {
            Some(round1(hours(
                f.complete_ms?.saturating_sub(f.complete_started_ms?),
            )))
        }),
        eta_hours,
    }
}

/// How the backfill stands, when the configuration has a `[message_cache]` table and a cache is
/// open, each channel with its label. Reads the file: call it on the blocking pool.
#[must_use]
pub fn history(state: &AppState) -> Option<HistoryReport> {
    let config = state.config.message_cache.as_ref()?;
    let cache = state
        .message_cache
        .cache()
        .filter(|cache| cache.is_open())?;
    let mut report = history_of(config, &cache);
    let labels: HashMap<String, String> = state
        .all_channels()
        .into_iter()
        .map(|channel| (channel.id.0, channel.label))
        .collect();
    let control = state.live.history();
    for channel in &mut report.channels {
        channel.label = labels.get(&channel.channel).cloned();
        let id = ChannelId(channel.channel.clone());
        let own = control.mode(&id);
        channel.mode_override = own.map(HistoryMode::as_str);
        channel.mode = effective_mode(config.backfill, own);
        channel.queue_position = control.position(&id);
    }
    report.providers = control.paces();
    // A channel walked whole on a server set to `recent` waits for room too.
    if report.paused.is_none()
        && report.channels.iter().any(|channel| channel.mode == "full")
        && cache.bytes_on_disk().saturating_mul(100)
            >= u64::from(config.max_megabytes) * 1024 * 1024 * FULL_STORAGE_PERCENT
    {
        report.paused = Some("storage");
    }
    Some(report)
}

/// How the backfill stands by `config`, from what `cache` holds. Reads the file.
#[must_use]
pub fn history_of(
    config: &MessageCacheConfig,
    cache: &crate::cache::MessageCache,
) -> HistoryReport {
    let now_ms = cache.now_ms();
    let stats = cache.channel_stats().served().unwrap_or_default();
    let bytes = cache.bytes_on_disk();
    let messages: u64 = stats.iter().map(|s| s.messages).sum();
    let channels: Vec<ChannelHistory> = stats
        .iter()
        .map(|stats| {
            let frontier = cache.frontier(&stats.chan).served().flatten();
            channel_history(stats, frontier.as_ref(), now_ms)
        })
        .collect();
    let full = config.backfill == CacheBackfill::Full;
    let paused = (full
        && bytes.saturating_mul(100)
            >= u64::from(config.max_megabytes) * 1024 * 1024 * FULL_STORAGE_PERCENT)
        .then_some("storage");
    HistoryReport {
        mode: config.backfill.as_str(),
        recent_days: config.recent_days,
        rewalk_hot_hours: config.rewalk_hot_hours,
        rewalk_hot_minutes: config.rewalk_hot_minutes,
        rewalk_recent_hours: config.rewalk_recent_hours,
        full_reads_per_minute: config.full_reads_per_minute,
        full_cycle_hours: config.full_cycle_hours,
        paused,
        bytes_per_message: (messages > 0).then(|| bytes / messages),
        providers: BTreeMap::new(),
        channels: channels
            .into_iter()
            .map(|channel| ChannelHistory {
                mode: config.backfill.as_str(),
                ..channel
            })
            .collect(),
    }
}

/// How far back the backfill reads a channel whose own mode is `own` on a server set to `server`:
/// `off` whatever the channel says, otherwise its own mode, otherwise the server's. `#602
/// history-settings-and-limits`.
#[must_use]
pub fn effective_mode(server: CacheBackfill, own: Option<HistoryMode>) -> &'static str {
    match (server, own) {
        (CacheBackfill::Off, _) => "off",
        (_, Some(own)) => own.as_str(),
        (server, None) => server.as_str(),
    }
}

impl HistoryReport {
    /// What the cache status in client-config says of it: how many channels the cache holds whole
    /// back to their beginnings, and the oldest message any full walk under way has reached.
    #[must_use]
    pub fn summary(&self) -> crate::contract::MessageCacheHistory {
        let complete = self
            .channels
            .iter()
            .filter(|channel| channel.complete)
            .count();
        crate::contract::MessageCacheHistory {
            mode: self.mode,
            channels: u64::try_from(self.channels.len()).unwrap_or(u64::MAX),
            complete: u64::try_from(complete).unwrap_or(u64::MAX),
            reached: self
                .channels
                .iter()
                .filter_map(|channel| channel.reached.clone())
                .min(),
            paused: self.paused,
        }
    }
}

/// One channel's history and the owner's say over it, as `GET /api/v1/channels/{id}/history`
/// answers, with `own` the channel's own mode as the store holds it and `settable` whether the
/// store can keep one. Reads the file when a cache is open: call it on the blocking pool. `#602
/// history-settings-and-limits`.
#[must_use]
pub fn channel_state(
    state: &AppState,
    channel: &ChannelId,
    own: Option<HistoryMode>,
    settable: bool,
) -> crate::contract::ChannelHistoryState {
    let control = state.live.history();
    let config = state.config.message_cache.as_ref();
    let server = config.map_or(CacheBackfill::Off, |config| config.backfill);
    let cache = state
        .message_cache
        .cache()
        .filter(|cache| cache.is_open());
    let wall_ms = jiff::Timestamp::now().as_millisecond();
    let chan = state.message_cache.chan_key(channel);
    let (stats, frontier, cache_now) = match (&cache, &chan) {
        (Some(cache), Some(chan)) => (
            cache
                .channel_stats()
                .served()
                .unwrap_or_default()
                .into_iter()
                .find(|stats| &stats.chan == chan),
            cache.frontier(chan).served().flatten(),
            cache.now_ms(),
        ),
        _ => (None, None, wall_ms),
    };
    let history = stats
        .as_ref()
        .map(|stats| channel_history(stats, frontier.as_ref(), cache_now));
    let key = state.providers.key_for(channel);
    let pace = key.as_deref().and_then(|key| control.pace(key));
    let effective = effective_mode(server, own);
    let storage = config.zip(cache.as_ref()).is_some_and(|(config, cache)| {
        effective == "full"
            && cache.bytes_on_disk().saturating_mul(100)
                >= u64::from(config.max_megabytes) * 1024 * 1024 * FULL_STORAGE_PERCENT
    });
    let (held_until_ms, held_reason) = match pace.as_ref() {
        Some(PaceReport {
            held_until_ms: Some(until),
            held_because,
            ..
        }) if *until > wall_ms => (Some(*until), *held_because),
        _ if storage => (None, Some(HoldReason::Storage)),
        _ => (None, None),
    };
    let iso_ms = |text: &Option<String>| {
        text.as_deref()
            .and_then(|text| text.parse::<jiff::Timestamp>().ok())
            .map(jiff::Timestamp::as_millisecond)
    };
    crate::contract::ChannelHistoryState {
        channel_id: channel.as_str().to_owned(),
        server_mode: server.as_str(),
        mode_override: own,
        mode: effective,
        settable,
        messages: history.as_ref().map_or(0, |h| h.messages),
        oldest_ms: history.as_ref().and_then(|h| iso_ms(&h.oldest)),
        newest_ms: history.as_ref().and_then(|h| iso_ms(&h.newest)),
        walking: history.as_ref().is_some_and(|h| h.walking),
        reached_ms: history.as_ref().and_then(|h| iso_ms(&h.reached)),
        complete: history.as_ref().is_some_and(|h| h.complete),
        complete_ms: history.as_ref().and_then(|h| iso_ms(&h.complete_at)),
        cycle_pages: history.as_ref().map_or(0, |h| h.cycle_pages),
        pages_per_hour: history.as_ref().and_then(|h| h.pages_per_hour),
        days_per_hour: history.as_ref().and_then(|h| h.days_per_hour),
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        eta_s: history
            .as_ref()
            .and_then(|h| h.eta_hours)
            .map(|hours| (hours * 3600.0).max(0.0).round() as u64),
        queue_position: control
            .position(channel)
            .map(|at| u32::try_from(at).unwrap_or(u32::MAX)),
        held_until_ms,
        held_reason,
        reads_per_minute: key.as_deref().map(|key| ceiling_of(state, key)),
        reads_last_minute: pace.as_ref().map(|pace| pace.reads_last_minute),
        now_ms: wall_ms,
    }
}
