//! The server's one handle on the message cache: a [`Sink`]. `#208 persistent-cache`.
//!
//! The handle is always present, in every server state, whether or not a cache is open behind it,
//! so a read site never has to ask whether the cache exists before it asks whether it may fill
//! it. Every fill starts with [`Sink::begin`], and only under a [`FillGrant`]: a read-scope caller
//! has none (see [`super::grant`]), so it never reaches the cache at all.
//!
//! # The route snapshot, and the startup guard
//!
//! The cache may hold rows only for the channels the server answers for, each under the provider
//! that owns it. [`Sink::set_routes`] is told that list every time it changes
//! (`AppState::refresh_routes`), and turns it into the [`RouteSnapshot`] the cache's writer gates
//! on and sweeps to: every channel and provider key off it is erased from the file.
//!
//! A sweep is only as good as the list it sweeps to. At startup the list is put together from the
//! configuration AND from the store (the channels added in the app, and the configured ones the
//! owner hid), and a store that failed to answer leaves it short: sweeping to it would erase every
//! channel the owner added. So nothing is sent to the cache until [`Sink::arm`] has been called,
//! which the server does only once both lists came back from the store. Unarmed, the sink keeps
//! the latest list and does nothing with it: no snapshot reaches the cache, so the cache sweeps
//! nothing and its writer refuses every row, and [`Sink::begin`] refuses every fill, for the life
//! of the process.
//!
//! # Offering what a read saw
//!
//! A [`Fill`] is begun before the read and offered once, after it, through the method for the
//! kind of read it was. [`super::observe`] decides what that read proves, by the shape of the
//! channel's provider; the fill supplies the observation time, the earlier of when the read began
//! and the `as_of` the provider dated its answer with, so a change pushed while the read ran, or
//! after the provider's copy was taken, is never undone by it. The rows offered are the provider's
//! own, before anything prepares them for a reader.
//!
//! # Changes from the live feed, and posts
//!
//! The live hub offers every change it publishes ([`Sink::offer_push`]) under the server's own
//! grant, while it holds the channel's feed lock and before any subscriber is sent it: a page that
//! saw a deletion and then reads the cache can never find the message still there for want of the
//! deletion being queued. The lock order is the feed lock, then the cache's sequencer, and nothing
//! here waits. A message this server posts is offered as it comes back from the provider
//! ([`Fill::offer_posted`]), also under the server's own grant.
//!
//! # The store's summaries
//!
//! A cache put behind the sink ([`Sink::install`]) is followed by a task that deletes the store's
//! summaries of messages the cache learns were edited or deleted, and of channels it erases that
//! are on the server's list under no provider at all ([`super::orphans`]). With no cache, there is
//! no task, and no summary is ever deleted on the cache's account.
//!
//! In this build nothing installs a cache outside the tests, so the slot stays empty and every
//! fill is refused.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use super::grant::{FillGrant, ServerFill};
use super::observe::{self, PageRead, Shape, TimelineRead};
use super::orphans::{self, Summaries};
use super::{ChanKey, MessageCache, Observation, RouteSnapshot, Stamp};
use crate::chat::RecentRead;
use crate::config::{Config, ProviderKind, RecentView};
use crate::live::LiveMessage;
use crate::model::{ChannelId, ChannelInfo, Message};
use crate::providers::ChatRouter;
use crate::threads::{TimelinePage, TimelineRequest};

/// The message cache as the server sees it: an empty slot, or an open cache, and the channels it
/// may hold.
#[derive(Debug, Default)]
pub struct Sink {
    slot: RwLock<Option<Arc<MessageCache>>>,
    /// Taken before the slot whenever both are held, never the other way round.
    routes: Mutex<Routes>,
    /// Every channel id on the server's list, under whatever provider: what the task that keeps
    /// the store's summaries in step asks before it forgets an erased channel's.
    listed: Arc<RwLock<HashSet<ChannelId>>>,
    /// The server's summaries, for that task. Attached once; with none attached, no task runs.
    summaries: OnceLock<Summaries>,
    /// How far that task has got with the cache installed now.
    orphans: Mutex<OrphanProgress>,
}

/// How many of the writer's events the summaries task has followed, against the count the cache
/// had sent when the task began.
#[derive(Debug, Default)]
struct OrphanProgress {
    handled: Arc<AtomicU64>,
    sent_before: u64,
}

/// What the sink knows about the channels the server answers for.
#[derive(Debug, Default)]
struct Routes {
    /// Whether the list may be sent to the cache: see the module documentation.
    armed: bool,
    /// The latest list, as the cache names its channels; `None` until the first is set.
    snapshot: Option<RouteSnapshot>,
    /// Each channel a fill may be begun for: the provider that owns it, and how that provider
    /// lists it.
    fills: HashMap<ChannelId, Route>,
}

/// How one channel is read: its provider's key, the shape of that provider, and whether its recent
/// reads go through a bridge's Flat view.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Route {
    key: String,
    shape: Shape,
    recent_via_flat: bool,
}

/// One upstream read's permission to offer what it sees, taken by [`Sink::begin`] before the read
/// starts.
///
/// It carries what has to be noted BEFORE the read: the stamp that lets the writer drop what the
/// read saw if the channel or the file is erased while it runs, and the local time the read
/// started, which is what decides whether what it saw is newer than what the cache already holds.
#[derive(Debug)]
pub struct Fill {
    grant: FillGrant,
    channel: ChannelId,
    chan: ChanKey,
    stamp: Stamp,
    observed_ms: i64,
    cache: Arc<MessageCache>,
    shape: Shape,
    recent_via_flat: bool,
}

impl Fill {
    /// The grant the fill was begun under.
    #[must_use]
    pub fn grant(&self) -> FillGrant {
        self.grant
    }

    /// The channel being read.
    #[must_use]
    pub fn channel(&self) -> &ChannelId {
        &self.channel
    }

    /// The channel as the cache names it: the provider that owns it, and its id.
    #[must_use]
    pub fn chan(&self) -> &ChanKey {
        &self.chan
    }

    /// The stamp taken when the fill began.
    #[must_use]
    pub fn stamp(&self) -> Stamp {
        self.stamp
    }

    /// The local time the read started, by the cache's clock.
    #[must_use]
    pub fn observed_ms(&self) -> i64 {
        self.observed_ms
    }

    /// The cache the fill offers to.
    #[must_use]
    pub fn cache(&self) -> &Arc<MessageCache> {
        &self.cache
    }

    /// How the channel's provider lists it.
    #[must_use]
    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// Offer a `/timeline` page, raw, as the provider answered `request`.
    pub fn offer_timeline(
        self,
        request: &TimelineRequest,
        read: TimelineRead,
        page: &TimelinePage,
    ) {
        let observed_ms = self.observed(page.as_of.as_deref());
        let observations = observe::timeline(self.shape, request, read, page, observed_ms);
        self.offer(observations);
    }

    /// Offer a read of the channel's most recent messages through `fetch_recent_dated`, which a
    /// bridge configured so answers from its Flat view.
    pub fn offer_recent_dated(self, read: &RecentRead) {
        let observed_ms = self.observed(read.as_of.as_deref());
        let observations = observe::window(self.shape, read, self.recent_via_flat, observed_ms);
        self.offer(observations);
    }

    /// Offer a read of the channel's most recent messages through `fetch_recent`, which no
    /// provider answers from a Flat view and none dates.
    pub fn offer_recent(self, messages: &[Message]) {
        let read = RecentRead {
            messages: messages.to_vec(),
            ..RecentRead::default()
        };
        let observations = observe::window(self.shape, &read, false, self.observed_ms);
        self.offer(observations);
    }

    /// Offer one `/page` step: the messages the provider returned inside the requested span,
    /// before any was cut to the page's size, and whether one was.
    pub fn offer_page(self, read: PageRead, messages: &[Message], truncated: bool) {
        let observations = observe::page(self.shape, read, messages, truncated, self.observed_ms);
        self.offer(observations);
    }

    /// Offer every message a `/count` walk read, from the newest page down.
    pub fn offer_walk(self, messages: &[Message]) {
        let observations = observe::walk(self.shape, messages, self.observed_ms);
        self.offer(observations);
    }

    /// Offer one message read by id from the thread `thread_id`.
    pub fn offer_single(self, message: &Message, thread_id: &str) {
        let observations = observe::single(self.shape, message, thread_id, self.observed_ms);
        self.offer(observations);
    }

    /// Offer one change from the live feed, observed when the fill began.
    pub fn offer_push(self, live: &LiveMessage) {
        let observation = observe::pushed(self.shape, live, self.observed_ms);
        self.offer(vec![observation]);
    }

    /// Offer what the live poller's walk forward from its cursor read: every message its pages
    /// returned, `after_ms` the provider time of the cursor it walked from, and `reached_end`
    /// whether it ended on a page shorter than it asked for, rather than at its page bound.
    pub fn offer_forward(self, messages: &[Message], after_ms: i64, reached_end: bool) {
        let observations = observe::forward(
            self.shape,
            messages,
            after_ms,
            reached_end,
            self.observed_ms,
        );
        self.offer(observations);
    }

    /// Offer one message this server posted, into the thread `thread_id` when it went to one.
    pub fn offer_posted(self, message: &Message, thread_id: Option<&str>) {
        let observations = observe::posted(self.shape, message, thread_id, self.observed_ms);
        self.offer(observations);
    }

    /// When the read saw the channel: when it began, or the provider's `as_of` when that is
    /// earlier. `as_of` is the wall clock of a provider on this host.
    fn observed(&self, as_of: Option<&str>) -> i64 {
        as_of
            .and_then(crate::clock::instant_ms)
            .map_or(self.observed_ms, |as_of| as_of.min(self.observed_ms))
    }

    /// Queue every observation, in order, under the stamp taken when the read began. A full queue
    /// drops one and marks the channel suspect; nothing here waits.
    fn offer(self, observations: Vec<Observation>) {
        for observation in observations {
            self.cache.offer(self.chan.clone(), self.stamp, observation);
        }
    }
}

impl Sink {
    /// A sink with no cache behind it: every fill is refused.
    #[must_use]
    pub fn off() -> Self {
        Self::default()
    }

    /// Have the store's summaries kept in step with every cache installed from now on: see
    /// [`super::orphans`]. Once per sink; the server attaches its own before it installs a cache.
    pub fn attach_summaries(&self, summaries: Summaries) {
        let _ = self.summaries.set(summaries);
    }

    /// Put `cache` behind the sink. If the sink is armed, the latest route snapshot is sent to it
    /// at once: the cache serves and accepts nothing until it has one.
    ///
    /// When the server's summaries are attached and a runtime is running, this first starts the
    /// task that deletes the store's summaries the cache learns are stale ([`super::orphans`]),
    /// so it hears of the very first sweep. It ends when the cache does.
    pub fn install(&self, cache: Arc<MessageCache>) {
        if let (Some(summaries), Ok(runtime)) =
            (self.summaries.get(), tokio::runtime::Handle::try_current())
        {
            let handled = Arc::new(AtomicU64::new(0));
            *self.orphans.lock().unwrap_or_else(PoisonError::into_inner) = OrphanProgress {
                handled: Arc::clone(&handled),
                sent_before: cache.stats().orphans_sent,
            };
            runtime.spawn(orphans::run(
                cache.subscribe_events(orphans::EVENT_QUEUE),
                summaries.clone(),
                Arc::clone(&self.listed),
                handled,
            ));
        }
        let routes = self.routes();
        *self.slot.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&cache));
        if routes.armed {
            if let Some(snapshot) = &routes.snapshot {
                cache.retain_only(snapshot.clone());
            }
        }
    }

    /// Take the cache out from behind the sink, leaving it empty. The cache itself is closed when
    /// the last handle on it is dropped.
    pub fn uninstall(&self) -> Option<Arc<MessageCache>> {
        let _routes = self.routes();
        self.slot
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The cache behind the sink, if any.
    #[must_use]
    pub fn cache(&self) -> Option<Arc<MessageCache>> {
        self.slot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Allow the route list to reach the cache, and send the latest one now. Called once the
    /// whole list is known: see the module documentation.
    pub fn arm(&self) {
        let mut routes = self.routes();
        routes.armed = true;
        if let (Some(cache), Some(snapshot)) = (self.cache(), &routes.snapshot) {
            cache.retain_only(snapshot.clone());
        }
    }

    /// Whether [`Sink::arm`] has been called.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.routes().armed
    }

    /// The channels the server answers for have changed to `channels`: name each under the
    /// provider `router` routes it to, and, once armed, send the cache the new snapshot, which
    /// sweeps every other channel and every provider key `router` does not have out of the file.
    ///
    /// A channel the router routes to no provider is left out: nothing can be read from it, so
    /// nothing may be kept for it either. Hidden channels are not in `channels` at all. A channel
    /// whose provider `config` does not describe stays in the snapshot but is never filled: what
    /// its reads prove depends on how that provider lists it.
    pub fn set_routes(&self, channels: &[ChannelInfo], router: &ChatRouter, config: &Config) {
        let mut fills = HashMap::with_capacity(channels.len());
        let mut chans = Vec::with_capacity(channels.len());
        for channel in channels {
            let Some(key) = router.key_for(&channel.id) else {
                continue;
            };
            chans.push(ChanKey::new(key.clone(), channel.id.as_str()));
            if let Some(provider) = config.provider(&key) {
                let recent_via_flat = match &provider.kind {
                    ProviderKind::Discord(discord) => discord.recent.view == RecentView::Flat,
                    ProviderKind::Slack(_) => false,
                };
                let route = Route {
                    shape: Shape::of(provider, channel.id.as_str()),
                    key,
                    recent_via_flat,
                };
                fills.insert(channel.id.clone(), route);
            }
        }
        let snapshot = RouteSnapshot::new(
            chans,
            router.entries().iter().map(|entry| entry.key.clone()),
        );
        *self.listed.write().unwrap_or_else(PoisonError::into_inner) =
            channels.iter().map(|channel| channel.id.clone()).collect();
        let mut routes = self.routes();
        routes.fills = fills;
        routes.snapshot = Some(snapshot.clone());
        if routes.armed {
            if let Some(cache) = self.cache() {
                cache.retain_only(snapshot);
            }
        }
    }

    /// Begin a fill of `channel` under `grant`, before its upstream read starts. `None` means
    /// nothing the read sees is offered: there is no cache, the sink is not armed, or the channel
    /// is not one the server answers for.
    #[must_use]
    pub fn begin(&self, grant: &FillGrant, channel: &ChannelId) -> Option<Fill> {
        let cache = self.cache()?;
        let route = {
            let routes = self.routes();
            if !routes.armed {
                return None;
            }
            routes.fills.get(channel)?.clone()
        };
        let chan = ChanKey::new(route.key, channel.as_str());
        let stamp = cache.stamp(&chan);
        let observed_ms = cache.now_ms();
        Some(Fill {
            grant: *grant,
            channel: channel.clone(),
            chan,
            stamp,
            observed_ms,
            cache,
            shape: route.shape,
            recent_via_flat: route.recent_via_flat,
        })
    }

    /// [`Sink::begin`] under the grant a request's caller holds: none at read scope.
    #[must_use]
    pub fn begin_for(&self, caller: crate::auth::Scope, channel: &ChannelId) -> Option<Fill> {
        FillGrant::for_caller(caller).and_then(|grant| self.begin(&grant, channel))
    }

    /// Offer one change the live hub is publishing to `channel`, from `producer`: the poller, or
    /// an adapter's pushed event.
    ///
    /// Called by [`crate::live::LiveHub`] while it holds the channel's feed lock and BEFORE any
    /// subscriber is sent the change, so a change a subscriber could have seen is already queued,
    /// with a sequence number, by the time anyone who saw it reads the cache. So it must never
    /// wait and never call back into the hub: the lock order is the feed lock, then the cache's
    /// sequencer. It takes the sink's own locks and the sequencer briefly and returns; a full
    /// queue drops the offer and marks the channel suspect, and the publish goes on regardless.
    pub(crate) fn offer_push(&self, channel: &ChannelId, live: &LiveMessage, producer: ServerFill) {
        if let Some(fill) = self.begin(&FillGrant::server(producer), channel) {
            fill.offer_push(live);
        }
    }

    /// Wait, up to `timeout`, until the summaries task has followed every event the cache behind
    /// the sink has sent so far. `true` at once with no cache, or no task. For tests, after the
    /// cache has been waited for ([`MessageCache::quiesce_blocking`]): a summary deleted, or
    /// rightly kept, is only known to be so once the task has had its turn.
    pub async fn orphans_settled(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let Some(cache) = self.cache() else {
                return true;
            };
            if self.summaries.get().is_none() {
                return true;
            }
            let (handled, sent_before) = {
                let progress = self.orphans.lock().unwrap_or_else(PoisonError::into_inner);
                (
                    progress.handled.load(Ordering::SeqCst),
                    progress.sent_before,
                )
            };
            if sent_before.saturating_add(handled) >= cache.stats().orphans_sent {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn routes(&self) -> std::sync::MutexGuard<'_, Routes> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::grant::ServerFill;
    use super::super::{CacheOptions, Read, Scope};
    use super::*;
    use crate::auth::Scope as Caller;

    #[test]
    fn a_sink_that_is_off_begins_no_fill_under_any_grant() {
        let sink = Sink::off();
        let channel = ChannelId("1000000000000000001".to_owned());
        let caller = FillGrant::for_caller(Caller::Write).expect("write scope may fill");
        assert!(sink.begin(&caller, &channel).is_none());
        for kind in ServerFill::ALL {
            assert!(
                sink.begin(&FillGrant::server(kind), &channel).is_none(),
                "{kind:?}"
            );
        }
    }

    fn open(dir: &crate::testing::TempDir) -> Arc<MessageCache> {
        Arc::new(
            MessageCache::open(CacheOptions::new(
                dir.path().join("private").join("cache.sqlite3"),
            ))
            .expect("a cache opens"),
        )
    }

    #[test]
    fn nothing_reaches_the_cache_until_the_sink_is_armed() {
        let dir = crate::testing::TempDir::new("sink-armed");
        let (state, _fake) = crate::testing::state();
        let sink = Sink::off();
        let cache = open(&dir);
        sink.install(Arc::clone(&cache));
        sink.set_routes(&state.all_channels(), &state.providers, &state.config);
        let read = ChannelId(crate::testing::READ_CHANNEL.to_owned());
        let chan = ChanKey::new(crate::config::LEGACY_DISCORD_PROVIDER_KEY, read.as_str());

        // Unarmed: no snapshot reached the cache, so it serves nothing and the sink fills nothing.
        assert!(!sink.is_armed());
        assert!(sink.begin_for(Caller::Write, &read).is_none());
        assert!(cache.barrier_blocking(Duration::from_secs(10)));
        assert_eq!(cache.applied_seq(), 0, "no sweep was queued");
        assert_eq!(cache.newest(&chan, &Scope::Main, 5), Read::Blocked);

        sink.arm();
        assert!(cache.barrier_blocking(Duration::from_secs(10)));
        assert_eq!(
            cache.newest(&chan, &Scope::Main, 5),
            Read::Served(Vec::new())
        );
        let fill = sink
            .begin_for(Caller::Write, &read)
            .expect("an armed sink fills");
        assert_eq!(fill.chan(), &chan);
        assert_eq!(fill.grant().label(), "caller");
        assert!(sink.begin_for(Caller::Read, &read).is_none(), "read scope");
        assert!(
            sink.begin_for(Caller::Write, &ChannelId("999".to_owned()))
                .is_none(),
            "a channel the server does not answer for"
        );
    }

    /// 2026-08-18T12:00:00Z.
    const T: i64 = 1_787_054_400_000;

    fn big_message(channel: &ChannelId, n: i64, body: &str) -> Message {
        let at = T + n * 1_000;
        Message {
            edited_timestamp: None,
            mentions: Vec::new(),
            thread: None,
            id: crate::model::MessageId::at_time_ms(at),
            channel_id: channel.clone(),
            author: "Ada".to_owned(),
            author_id: crate::model::UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: crate::clock::iso_from_ms(at),
            spoken_time: String::new(),
            reply_to: None,
            content: body.to_owned(),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            reactions: None,
            heading: None,
        }
    }

    #[test]
    fn a_publish_with_a_full_queue_never_waits_it_drops_the_offer_and_marks_the_channel_suspect() {
        let dir = crate::testing::TempDir::new("sink-full-queue");
        let (state, _fake) = crate::testing::state();
        let clock = crate::testing::ManualClock::at(T + 60 * 60 * 1_000);
        let state = crate::testing::with_message_cache(state, dir.path(), clock);
        let cache = state.message_cache.cache().expect("installed");
        assert!(cache.quiesce_blocking(Duration::from_secs(10)));
        let channel = ChannelId(crate::testing::READ_CHANNEL.to_owned());
        let chan = ChanKey::new(crate::config::LEGACY_DISCORD_PROVIDER_KEY, channel.as_str());

        // The writer stands still, so nothing leaves the queue, and every publish offers a few
        // megabytes: the queue's byte budget runs out after a handful.
        let resume = cache.pause_writer();
        let body = "x".repeat(3 * 1024 * 1024);
        let mut published = 0;
        while !cache.is_suspect(&chan) {
            assert!(published < 40, "the hub never offered the cache anything");
            let started = std::time::Instant::now();
            state
                .live
                .publish(&channel, big_message(&channel, published, &body));
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "a publish waited on the cache"
            );
            published += 1;
        }
        assert!(published > 1, "some offers fitted before one was dropped");
        let stats = cache.stats();
        assert_eq!(
            stats.ops_dropped, 1,
            "the offer that did not fit was dropped"
        );
        // The publish itself went on: every message is in the channel's feed.
        assert_eq!(
            state.live.subscribe(&channel, None).replay.len(),
            usize::try_from(published).expect("small")
        );

        drop(resume);
        assert!(cache.quiesce_blocking(Duration::from_secs(30)));
        assert_eq!(
            cache.counts().served().expect("counts").messages,
            u64::try_from(published - 1).expect("small"),
            "everything queued was applied, and the dropped offer was not"
        );
    }

    #[test]
    fn a_cache_installed_after_arming_gets_the_latest_snapshot_at_once() {
        let dir = crate::testing::TempDir::new("sink-install");
        let (state, _fake) = crate::testing::state();
        let sink = Sink::off();
        sink.set_routes(&state.all_channels(), &state.providers, &state.config);
        sink.arm();
        let cache = open(&dir);
        sink.install(Arc::clone(&cache));
        assert!(cache.barrier_blocking(Duration::from_secs(10)));
        let chan = ChanKey::new(
            crate::config::LEGACY_DISCORD_PROVIDER_KEY,
            crate::testing::WRITE_CHANNEL,
        );
        assert_eq!(
            cache.newest(&chan, &Scope::Main, 5),
            Read::Served(Vec::new())
        );
        assert!(sink.uninstall().is_some());
        assert!(sink.cache().is_none());
        assert!(sink
            .begin_for(
                Caller::Write,
                &ChannelId(crate::testing::WRITE_CHANNEL.to_owned())
            )
            .is_none());
    }
}
