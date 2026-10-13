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
//! In this build nothing installs a cache outside the tests, so the slot stays empty and every
//! fill is refused.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use super::grant::FillGrant;
use super::{ChanKey, MessageCache, RouteSnapshot, Stamp};
use crate::model::{ChannelId, ChannelInfo};
use crate::providers::ChatRouter;

/// The message cache as the server sees it: an empty slot, or an open cache, and the channels it
/// may hold.
#[derive(Debug, Default)]
pub struct Sink {
    slot: RwLock<Option<Arc<MessageCache>>>,
    /// Taken before the slot whenever both are held, never the other way round.
    routes: Mutex<Routes>,
}

/// What the sink knows about the channels the server answers for.
#[derive(Debug, Default)]
struct Routes {
    /// Whether the list may be sent to the cache: see the module documentation.
    armed: bool,
    /// The latest list, as the cache names its channels; `None` until the first is set.
    snapshot: Option<RouteSnapshot>,
    /// Each routed channel's provider key, for naming the channel a fill is for.
    providers: HashMap<ChannelId, String>,
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
}

impl Sink {
    /// A sink with no cache behind it: every fill is refused.
    #[must_use]
    pub fn off() -> Self {
        Self::default()
    }

    /// Put `cache` behind the sink. If the sink is armed, the latest route snapshot is sent to it
    /// at once: the cache serves and accepts nothing until it has one.
    pub fn install(&self, cache: Arc<MessageCache>) {
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
    /// nothing may be kept for it either. Hidden channels are not in `channels` at all.
    pub fn set_routes(&self, channels: &[ChannelInfo], router: &ChatRouter) {
        let mut providers = HashMap::with_capacity(channels.len());
        let mut chans = Vec::with_capacity(channels.len());
        for channel in channels {
            let Some(key) = router.key_for(&channel.id) else {
                continue;
            };
            chans.push(ChanKey::new(key.clone(), channel.id.as_str()));
            providers.insert(channel.id.clone(), key);
        }
        let snapshot = RouteSnapshot::new(
            chans,
            router.entries().iter().map(|entry| entry.key.clone()),
        );
        let mut routes = self.routes();
        routes.providers = providers;
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
        let key = {
            let routes = self.routes();
            if !routes.armed {
                return None;
            }
            routes.providers.get(channel)?.clone()
        };
        let chan = ChanKey::new(key, channel.as_str());
        let stamp = cache.stamp(&chan);
        let observed_ms = cache.now_ms();
        Some(Fill {
            grant: *grant,
            channel: channel.clone(),
            chan,
            stamp,
            observed_ms,
            cache,
        })
    }

    /// [`Sink::begin`] under the grant a request's caller holds: none at read scope.
    #[must_use]
    pub fn begin_for(&self, caller: crate::auth::Scope, channel: &ChannelId) -> Option<Fill> {
        FillGrant::for_caller(caller).and_then(|grant| self.begin(&grant, channel))
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
        sink.set_routes(&state.all_channels(), &state.providers);
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

    #[test]
    fn a_cache_installed_after_arming_gets_the_latest_snapshot_at_once() {
        let dir = crate::testing::TempDir::new("sink-install");
        let (state, _fake) = crate::testing::state();
        let sink = Sink::off();
        sink.set_routes(&state.all_channels(), &state.providers);
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
