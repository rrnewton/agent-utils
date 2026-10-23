//! Two background readers that keep the live feed and the message cache honest about what a
//! provider's event stream leaves out: the relist (`#452 slack-live-ingest`) and the backfill
//! (`#453 cache-backfill`).
//!
//! # The relist
//!
//! A provider's event stream carries new messages, and the server now publishes them as they
//! arrive ([`crate::live_tail`]). It does not carry edits, deletions or other bots' posts. The
//! live poller's walk forward from its cursor publishes the other bots' posts, and nothing at all
//! published an edit or a deletion of a message already out until a page read it again.
//!
//! So every `[ingest] relist_seconds`, each channel of a provider that takes pushed messages has
//! its newest page read once, and each of its threads a reply was pushed in recently has its
//! newest page read too (at most `[ingest] relist_threads` a channel). Each read is offered to
//! the message cache like any other server read, under the refresher's grant, and compared with
//! what the hub gave readers ([`crate::live::LiveHub::publish_relisted`]): a message the read
//! holds differently goes out as an update, one gone from inside the read's span as a deletion.
//! A stream that says it lost records asks for its channels' relist at once
//! ([`crate::live::LiveHub::request_relist`]).
//!
//! What it costs: one newest-page read per channel per interval, plus one per recently active
//! thread, one channel at a time. A read that fails is left for the next pass.
//!
//! # The backfill
//!
//! `[message_cache] backfill = "adaptive"` (the default) walks each cached channel's own timeline
//! from its newest page back through older pages, offering each to the cache under the backfill's
//! grant, until the provider says there is nothing older, the pages reach the cache's age limit,
//! or [`BACKFILL_PAGES`] pages have been read; the whole walk is then offered as one listing, as
//! of when it began, which is what proves the span its older pages cover. A cache that holds a channel's newest pages can
//! prove a newest page whole, and the shadow ([`crate::cache::shadow`]) then counts it servable.
//! It walks again every [`BACKFILL_REWALK`], which also keeps what it holds revalidated
//! ([`crate::cache::REVALIDATE_WITHIN_MS`]).
//!
//! It reads one page at a time, at least [`BACKFILL_SPACING`] apart, and only while the
//! provider has had nothing else running for [`BACKFILL_IDLE`]
//! ([`crate::upstream_profile::ProviderProfile::idle_for`]): a page reader, the poller and a
//! voice call always go first. `backfill = "off"` makes no call at all, and with no cache open
//! there is nothing to fill.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::cache::observe::TimelineRead;
use crate::cache::{FillGrant, ServerFill};
use crate::chat::{ChatClient as _, ChatError};
use crate::model::{ChannelId, ChannelInfo};
use crate::read_reason::ReadReason;
use crate::state::AppState;
use crate::threads::{TimelineRequest, TimelineView};

/// How recently a reply must have been pushed in a thread for the relist to read that thread:
/// this many relist intervals, and never less than [`ACTIVE_THREAD_FLOOR`].
pub const ACTIVE_THREAD_INTERVALS: u32 = 4;
/// The least time a thread stays recently active after a pushed reply.
pub const ACTIVE_THREAD_FLOOR: Duration = Duration::from_secs(10 * 60);
/// The pause between one channel's relist and the next's.
pub const RELIST_SPACING: Duration = Duration::from_millis(250);
/// How often a relist loop looks for a relist asked for early.
pub const RELIST_WAKE: Duration = Duration::from_secs(1);
/// How long before a relist read began a message must have been created for its absence from
/// the read to count as a deletion: a provider, or a bridge in front of it, may take a moment to
/// list a message it has just accepted.
pub const RELIST_SETTLE: Duration = Duration::from_secs(30);

/// How a relist reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelistOptions {
    /// Rows each read asks for.
    pub limit: u16,
    /// The most recently active threads of one channel a pass reads.
    pub threads: usize,
    /// The relist interval, which decides how long a thread stays recently active.
    pub interval: Duration,
    /// [`RELIST_SETTLE`], except in a test.
    pub settle: Duration,
}

/// The most pages one walk of the backfill reads of one channel.
pub const BACKFILL_PAGES: usize = 20;
/// The least time between two backfill reads.
pub const BACKFILL_SPACING: Duration = Duration::from_secs(1);
/// How long the provider must have had nothing else running before the backfill reads.
pub const BACKFILL_IDLE: Duration = Duration::from_secs(2);
/// How long a channel whose backfill read failed waits before it is read again.
pub const BACKFILL_RETRY: Duration = Duration::from_secs(10 * 60);
/// How long after a walk of every channel ends the backfill walks them all again.
pub const BACKFILL_REWALK: Duration = Duration::from_secs(6 * 60 * 60);

/// What one relist pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RelistPass {
    /// Scopes read: newest pages and threads.
    pub reads: usize,
    /// Reads that failed.
    pub failed: usize,
    /// Updates published.
    pub updates: usize,
    /// Deletions published.
    pub deletes: usize,
}

/// The channels a relist reads: the configured channels of every provider that takes pushed
/// messages.
fn relisted_channels(state: &AppState) -> Vec<ChannelInfo> {
    state
        .all_channels()
        .into_iter()
        .filter(|channel| {
            state
                .providers
                .client_for(&channel.id)
                .is_some_and(|client| client.takes_pushed_messages(&channel.id))
        })
        .collect()
}

/// Read one scope of `channel` again, offer the read to the cache, and publish what changed.
async fn relist_scope(
    state: &AppState,
    channel: &ChannelId,
    request: TimelineRequest,
    settle: Duration,
) -> Result<(usize, usize), ChatError> {
    let fill = state
        .message_cache
        .begin(&FillGrant::server(ServerFill::Refresher), channel);
    let began_ms = jiff::Timestamp::now().as_millisecond();
    let page = state.providers.fetch_timeline(channel, &request).await?;
    if let Some(fill) = fill {
        fill.offer_timeline(&request, TimelineRead::Newest, &page);
    }
    // A page the provider served from a copy it could not bring up to date, or with a notice,
    // proves nothing past its own newest message.
    let proven_until_ms = if page.as_of.is_some() || page.notice.is_some() {
        0
    } else {
        began_ms.saturating_sub(i64::try_from(settle.as_millis()).unwrap_or(i64::MAX))
    };
    Ok(state.live.publish_relisted(
        channel,
        &page.messages,
        request.thread_id.as_deref(),
        proven_until_ms,
    ))
}

/// Relist one channel: its newest page, and each thread a reply was pushed in since `since_ms`,
/// at most `threads` of them.
async fn relist_channel(
    state: &AppState,
    channel: &ChannelId,
    options: RelistOptions,
    since_ms: i64,
    pass: &mut RelistPass,
) {
    let RelistOptions {
        limit,
        threads,
        settle,
        ..
    } = options;
    let mut requests = vec![TimelineRequest {
        view: TimelineView::Main,
        limit,
        ..TimelineRequest::default()
    }];
    // A channel narrowed to one thread reads that thread as its own timeline already.
    let whole = crate::slack::ids::ChannelRef::parse(channel.as_str())
        .is_some_and(|target| target.thread_ts.is_none());
    if whole {
        requests.extend(
            state
                .live
                .active_threads(channel, since_ms)
                .into_iter()
                .take(threads)
                .map(|thread| TimelineRequest {
                    view: TimelineView::Thread,
                    thread_id: Some(thread),
                    limit,
                    ..TimelineRequest::default()
                }),
        );
    }
    for request in requests {
        pass.reads += 1;
        match relist_scope(state, channel, request, settle).await {
            Ok((updates, deletes)) => {
                pass.updates += updates;
                pass.deletes += deletes;
            }
            Err(error) => {
                pass.failed += 1;
                tracing::debug!(channel = %channel, %error, "a relist read failed; the next pass reads it");
            }
        }
    }
}

/// One relist pass over `channels`, or every relisted channel when `None`, at `now_ms`.
pub async fn relist_once(
    state: &AppState,
    channels: Option<&BTreeSet<ChannelId>>,
    options: RelistOptions,
    now_ms: i64,
) -> RelistPass {
    let active = options
        .interval
        .saturating_mul(ACTIVE_THREAD_INTERVALS)
        .max(ACTIVE_THREAD_FLOOR);
    let since_ms = now_ms.saturating_sub(i64::try_from(active.as_millis()).unwrap_or(i64::MAX));
    let mut pass = RelistPass::default();
    let mut first = true;
    for channel in relisted_channels(state) {
        if channels.is_some_and(|wanted| !wanted.contains(&channel.id)) {
            continue;
        }
        if !first {
            tokio::time::sleep(RELIST_SPACING).await;
        }
        first = false;
        ReadReason::Relist
            .scope(relist_channel(
                state,
                &channel.id,
                options,
                since_ms,
                &mut pass,
            ))
            .await;
    }
    if pass.updates + pass.deletes > 0 {
        tracing::info!(
            reads = pass.reads,
            updates = pass.updates,
            deletes = pass.deletes,
            "the relist found edits or deletions the event stream did not carry"
        );
    }
    pass
}

/// Relist every channel a provider's stream feeds every `interval`, and any channel asked for
/// early as soon as it is asked for. Never returns; spawned by `main` when `[ingest]
/// relist_seconds` is set.
pub async fn relist_forever(state: AppState, interval: Duration, limit: u16, threads: usize) {
    let options = RelistOptions {
        limit,
        threads,
        interval,
        settle: RELIST_SETTLE,
    };
    let mut next = tokio::time::Instant::now() + interval;
    loop {
        let wanted = state.live.take_relist_requests();
        let now_ms = jiff::Timestamp::now().as_millisecond();
        if tokio::time::Instant::now() >= next {
            relist_once(&state, None, options, now_ms).await;
            next = tokio::time::Instant::now() + interval;
        } else if !wanted.is_empty() {
            relist_once(&state, Some(&wanted), options, now_ms).await;
        }
        tokio::time::sleep(RELIST_WAKE.min(interval)).await;
    }
}

/// Where the backfill stands in one channel.
#[derive(Debug, Default)]
struct Walk {
    /// The continuation to read next; `None` before the newest page has been read.
    before: Option<String>,
    /// The fill begun before the walk's newest page was read, and every message the walk has read
    /// since, newest page first: offered together, once the walk ends, as one listing of the
    /// channel's own timeline from its oldest message to its newest end, as of when the walk
    /// began ([`crate::cache::Fill::offer_walk`]). Each page is offered as it is read too, but an
    /// older page alone is only rows: its span is proven by the walk it continues.
    listing: Option<crate::cache::Fill>,
    read: Vec<crate::model::Message>,
    /// Whether any page of the walk came from a copy the provider could not bring up to date
    /// (`as_of`) or carried a notice: then the walk proves nothing, and only its rows are offered.
    unproven: bool,
    /// Pages read in this walk.
    pages: usize,
    /// Whether this walk of the channel is over.
    done: bool,
    /// Not before this instant, after a failure.
    retry_at: Option<tokio::time::Instant>,
}

/// The `[message_cache]` table, when it asks for the backfill.
fn backfill_config(state: &AppState) -> Option<&crate::config::MessageCacheConfig> {
    state
        .config
        .message_cache
        .as_ref()
        .filter(|config| config.backfill == crate::config::CacheBackfill::Adaptive)
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
    /// Every channel's walk is over, or waiting after a failure.
    Idle,
    /// The backfill is off, or there is no cache to fill.
    Off,
}

/// The backfill's state across steps: one walk per channel.
#[derive(Debug, Default)]
pub struct Backfill {
    walks: BTreeMap<ChannelId, Walk>,
    turn: usize,
}

impl Backfill {
    /// A backfill that has walked nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start every channel's walk again from its newest page.
    pub fn rewalk(&mut self) {
        self.walks.clear();
    }

    /// Read the next page of the next channel whose walk is not over, if its provider is idle.
    pub async fn step(&mut self, state: &AppState, limit: u16, idle: Duration) -> BackfillStep {
        let Some(config) = backfill_config(state) else {
            return BackfillStep::Off;
        };
        let Some(cache) = state.message_cache.cache().filter(|cache| cache.is_open()) else {
            return BackfillStep::Off;
        };
        let channels: Vec<ChannelId> = state
            .all_channels()
            .into_iter()
            .map(|channel| channel.id)
            .filter(|channel| state.message_cache.chan_key(channel).is_some())
            .collect();
        let listed: BTreeSet<&ChannelId> = channels.iter().collect();
        self.walks.retain(|channel, _| listed.contains(channel));
        let now = tokio::time::Instant::now();
        let ready = |walk: Option<&Walk>| {
            walk.is_none_or(|walk| !walk.done && walk.retry_at.is_none_or(|at| at <= now))
        };
        let count = channels.len();
        let Some(channel) = (0..count)
            .map(|offset| &channels[(self.turn + offset) % count.max(1)])
            .find(|channel| ready(self.walks.get(*channel)))
            .cloned()
        else {
            return BackfillStep::Idle;
        };
        let busy = state
            .providers
            .key_for(&channel)
            .and_then(|key| state.providers.profile().find(&key))
            .is_some_and(|profile| profile.idle_for().is_none_or(|for_| for_ < idle));
        // The turn moves on either way, so one provider that is never idle does not hold back
        // every other provider's channels.
        self.turn = self.turn.wrapping_add(1);
        if busy {
            return BackfillStep::Busy;
        }
        let walk = self.walks.entry(channel.clone()).or_default();
        let request = TimelineRequest {
            view: TimelineView::Main,
            before: walk.before.clone(),
            limit,
            ..TimelineRequest::default()
        };
        let read = if walk.before.is_some() {
            TimelineRead::Before
        } else {
            TimelineRead::Newest
        };
        let fill = state
            .message_cache
            .begin(&FillGrant::server(ServerFill::Backfill), &channel);
        if walk.before.is_none() {
            walk.listing = state
                .message_cache
                .begin(&FillGrant::server(ServerFill::Backfill), &channel);
            walk.read.clear();
            walk.unproven = false;
        }
        let answer = ReadReason::Backfill
            .scope(state.providers.fetch_timeline(&channel, &request))
            .await;
        let page = match answer {
            Ok(page) => page,
            Err(error) => {
                tracing::debug!(channel = %channel, %error, "a backfill read failed; it is tried again later");
                walk.retry_at = Some(tokio::time::Instant::now() + BACKFILL_RETRY);
                return BackfillStep::Failed;
            }
        };
        if let Some(fill) = fill {
            fill.offer_timeline(&request, read, &page);
        }
        walk.unproven |= page.as_of.is_some() || page.notice.is_some();
        walk.read.extend(page.messages.iter().cloned());
        walk.pages += 1;
        walk.retry_at = None;
        let cutoff =
            crate::cache::Limits::from_config(config).cutoff_ms(channel.as_str(), cache.now_ms());
        let past_cutoff = page
            .messages
            .first()
            .and_then(|oldest| oldest.id.created_at_ms())
            .is_some_and(|oldest| oldest < cutoff);
        walk.before = page.next_before.clone();
        walk.done =
            !page.has_more || walk.before.is_none() || past_cutoff || walk.pages >= BACKFILL_PAGES;
        if walk.done {
            let read = std::mem::take(&mut walk.read);
            if let Some(listing) = walk.listing.take() {
                if !walk.unproven {
                    listing.offer_walk(&read);
                }
            }
        }
        BackfillStep::Read
    }
}

/// Walk every cached channel's history into the message cache, forever, a page at a time while
/// providers are idle. Spawned by `main` when a cache is configured; returns at once when the
/// backfill is off.
pub async fn backfill_forever(state: AppState, limit: u16) {
    if backfill_config(&state).is_none() {
        return;
    }
    let mut backfill = Backfill::new();
    let mut idle_since: Option<tokio::time::Instant> = None;
    loop {
        match backfill.step(&state, limit, BACKFILL_IDLE).await {
            BackfillStep::Read | BackfillStep::Failed => {
                idle_since = None;
                tokio::time::sleep(BACKFILL_SPACING).await;
            }
            BackfillStep::Busy => {
                idle_since = None;
                tokio::time::sleep(BACKFILL_SPACING).await;
            }
            // The owner turned the cache off, or it is not open yet: look again in a while.
            BackfillStep::Off => {
                idle_since = None;
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            BackfillStep::Idle => {
                let since = *idle_since.get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() >= BACKFILL_REWALK {
                    backfill.rewalk();
                    idle_since = None;
                }
                tokio::time::sleep(BACKFILL_SPACING.max(Duration::from_secs(30))).await;
            }
        }
    }
}
