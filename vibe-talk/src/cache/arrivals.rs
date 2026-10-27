//! How each recent message reached the message cache, in memory only: what the shadow's
//! diagnostics name beside a message its comparison found different (`#497
//! cache-shadow-missing-row`).
//!
//! The file records when a row was last seen and by which read of its scope's newest end, but not
//! what offered it: a page's read, the relist, the backfill, the event stream, the poller, a post
//! of this server's own. A message an answer lacked may never have been offered at all, or offered
//! only as a push, or only into another scope; which of those it was is most of what such a line
//! has to say. So every fill notes, as it offers, the ids it offers and the grant it offers them
//! under ([`super::FillGrant::label`]): the first offer of each id and the latest.
//!
//! Ids and times only, never text, kept for at most [`PER_CHANNEL`] messages a channel, the
//! earliest offered dropped first, and only for messages written in the last [`RECENT_MS`]: a
//! backfill walk of older history does not push out what was written today, nor does a deletion,
//! which only marks an id already recorded. Only what was queued is recorded: an offer the queue
//! dropped never reached the file. An erase leaves the record as it was: it names ids, and only
//! the diagnostics read it. Nothing decides on it, and it is gone when the process ends.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use super::rows::{created_ms, ChanKey};
use super::{lock, Observation, Push};

/// The most messages a channel's record holds.
pub const PER_CHANNEL: usize = 512;
/// How recently a message must have been written, by the time it is offered, to be recorded.
pub const RECENT_MS: i64 = 6 * 60 * 60 * 1000;

/// One offer of one message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    /// Who offered it: a [`super::FillGrant::label`].
    pub by: &'static str,
    /// What kind of observation carried it: `listing`, `rows`, `push`, `push-delete` or `delete`.
    pub kind: &'static str,
    /// When it was offered, local time.
    pub offered_ms: i64,
    /// The time the observation was offered as of: when its read began, or its push was published.
    pub observed_ms: i64,
}

/// The first offer of a message this process recorded, and the latest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrived {
    /// The first.
    pub first: Arrival,
    /// The latest: the first, when there was only one.
    pub latest: Arrival,
}

#[derive(Debug, Default)]
struct Channel {
    by_id: HashMap<String, Arrived>,
    /// Ids in the order they were first offered, for dropping the earliest.
    order: VecDeque<String>,
}

/// Every channel's record. See the module documentation.
#[derive(Debug, Default)]
pub struct Arrivals {
    channels: Mutex<HashMap<ChanKey, Channel>>,
}

/// What one observation offers, by id, ready to be recorded once the offer is queued
/// ([`Arrivals::noted`]).
#[derive(Debug, Default)]
pub struct Noted {
    /// Each id, its offer, and whether it may start a record of its own: a deletion only marks
    /// an id already recorded, so a delta's list of old ids never pushes out today's.
    ids: Vec<(String, Arrival, bool)>,
}

impl Arrivals {
    /// What `observation` offers, offered by `by` at local time `offered_ms`: built before the
    /// offer, which takes the observation, and recorded only once it was queued
    /// ([`Arrivals::record`]): an offer dropped never reached the file.
    #[must_use]
    pub fn noted(observation: &Observation, by: &'static str, offered_ms: i64) -> Noted {
        let recent_from = offered_ms.saturating_sub(RECENT_MS);
        let arrival = |kind: &'static str, observed_ms: i64| Arrival {
            by,
            kind,
            offered_ms,
            observed_ms,
        };
        let recent = |rows: &[super::Incoming], at: Arrival| -> Vec<(String, Arrival, bool)> {
            rows.iter()
                .filter(|row| created_ms(&row.message).is_some_and(|ms| ms >= recent_from))
                .map(|row| (row.message.id.as_str().to_owned(), at, true))
                .collect()
        };
        let ids = match observation {
            Observation::Listing(listing) => {
                recent(&listing.rows, arrival("listing", listing.observed_ms))
            }
            Observation::Rows {
                rows, observed_ms, ..
            } => recent(rows, arrival("rows", *observed_ms)),
            Observation::Push { push, observed_ms } => match push {
                Push::Upsert { message, .. } => vec![(
                    message.message.id.as_str().to_owned(),
                    arrival("push", *observed_ms),
                    true,
                )],
                Push::Delete { id, .. } => vec![(
                    id.as_str().to_owned(),
                    arrival("push-delete", *observed_ms),
                    false,
                )],
            },
            Observation::Deleted { ids, observed_ms } => {
                let at = arrival("delete", *observed_ms);
                ids.iter()
                    .map(|id| (id.as_str().to_owned(), at, false))
                    .collect()
            }
            Observation::Threads { .. } | Observation::Frontier(_) => Vec::new(),
        };
        Noted { ids }
    }

    /// Record what [`Arrivals::noted`] found of `chan`. Brief: one lock, one map entry an id.
    pub fn record(&self, chan: &ChanKey, noted: Noted) {
        if noted.ids.is_empty() {
            return;
        }
        let mut channels = lock(&self.channels);
        let channel = channels.entry(chan.clone()).or_default();
        for (id, at, starts) in noted.ids {
            if let Some(arrived) = channel.by_id.get_mut(&id) {
                arrived.latest = at;
                continue;
            }
            if !starts {
                continue;
            }
            channel.order.push_back(id.clone());
            channel.by_id.insert(
                id,
                Arrived {
                    first: at,
                    latest: at,
                },
            );
            while channel.order.len() > PER_CHANNEL {
                if let Some(oldest) = channel.order.pop_front() {
                    channel.by_id.remove(&oldest);
                }
            }
        }
    }

    /// [`Arrivals::noted`] and [`Arrivals::record`] at once, for an offer known to be queued.
    pub fn note(
        &self,
        chan: &ChanKey,
        by: &'static str,
        observation: &Observation,
        offered_ms: i64,
    ) {
        self.record(chan, Self::noted(observation, by, offered_ms));
    }

    /// What this process recorded of how message `id` of `chan` reached the cache.
    #[must_use]
    pub fn of(&self, chan: &ChanKey, id: &str) -> Option<Arrived> {
        lock(&self.channels)
            .get(chan)
            .and_then(|channel| channel.by_id.get(id))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::fixtures::message;
    use crate::cache::{Incoming, Listing, Scope};
    use crate::model::MessageId;

    /// 2026-08-18T12:00:00Z.
    const T: i64 = 1_787_054_400_000;

    fn chan() -> ChanKey {
        ChanKey::new("slack", "C1")
    }

    fn listing(ids: &[(&str, i64)], observed_ms: i64) -> Observation {
        Observation::Listing(Listing {
            scope: Scope::Main,
            rows: ids
                .iter()
                .map(|(id, at)| Incoming::new(message(id, "ada", "text", *at), Scope::Main))
                .collect(),
            observed_ms,
            proof: None,
        })
    }

    #[test]
    fn the_first_offer_and_the_latest_are_kept_by_who_offered_them() {
        let arrivals = Arrivals::default();
        let push = Observation::Push {
            push: Push::Upsert {
                message: Box::new(Incoming::new(message("m1", "ada", "hi", T), Scope::Main)),
                update: false,
                partial: false,
            },
            observed_ms: T + 1_000,
        };
        arrivals.note(&chan(), "stream", &push, T + 1_000);
        arrivals.note(
            &chan(),
            "refresher",
            &listing(&[("m1", T)], T + 5_000),
            T + 9_000,
        );
        let arrived = arrivals.of(&chan(), "m1").expect("recorded");
        assert_eq!((arrived.first.by, arrived.first.kind), ("stream", "push"));
        assert_eq!(
            (
                arrived.latest.by,
                arrived.latest.kind,
                arrived.latest.observed_ms
            ),
            ("refresher", "listing", T + 5_000)
        );
        assert_eq!(arrivals.of(&chan(), "m2"), None, "never offered");
        let unknown = Observation::Deleted {
            ids: vec![MessageId("m2".to_owned())],
            observed_ms: T + 9_500,
        };
        arrivals.note(&chan(), "caller", &unknown, T + 9_500);
        assert_eq!(
            arrivals.of(&chan(), "m2"),
            None,
            "a deletion only marks an id already recorded"
        );
        assert_eq!(arrivals.of(&ChanKey::new("slack", "C2"), "m1"), None);
        let deleted = Observation::Deleted {
            ids: vec![MessageId("m1".to_owned())],
            observed_ms: T + 10_000,
        };
        arrivals.note(&chan(), "caller", &deleted, T + 10_000);
        assert_eq!(
            arrivals.of(&chan(), "m1").expect("recorded").latest.kind,
            "delete"
        );
    }

    #[test]
    fn old_history_is_not_recorded_and_a_full_channel_drops_its_earliest() {
        let arrivals = Arrivals::default();
        let now = T + RECENT_MS + 60_000;
        arrivals.note(&chan(), "backfill", &listing(&[("old", T)], now), now);
        assert_eq!(
            arrivals.of(&chan(), "old"),
            None,
            "written over six hours before"
        );
        for n in 0..=PER_CHANNEL {
            let id = format!("r{n}");
            arrivals.note(&chan(), "caller", &listing(&[(&id, now)], now), now);
        }
        assert_eq!(
            arrivals.of(&chan(), "r0"),
            None,
            "the earliest offered went"
        );
        assert!(arrivals.of(&chan(), "r1").is_some());
        assert!(arrivals.of(&chan(), &format!("r{PER_CHANNEL}")).is_some());
    }
}
