//! What the shadow's rules decide over what the writer actually leaves in the file: the week's
//! revalidation, a short read of the newest end, a restart, the unsettled band and a write that
//! fails, each ending in the question a later milestone would answer from the cache. `#208
//! persistent-cache`, milestone `cache-fill`.
//!
//! Each test is a review's probe kept as a test: it reads the cache back through
//! [`MessageCache::shadow_view`] and decides with [`shadow::decide`], as the sink does.

use std::sync::atomic::AtomicI64;
use std::sync::Arc;
use std::time::Duration;

use super::coverage::Source;
use super::fixtures::{self, message, newest_proof, reply, Fixture, ManualClock, T0};
use super::tests::{m, STEP};
use super::*;
use crate::model::Message;

const DAY: i64 = 86_400_000;
const HOUR: i64 = 3_600_000;
const WAIT: Duration = Duration::from_secs(30);

fn facts(
    asked_ms: i64,
    live_fed: bool,
    fed_since_ms: Option<i64>,
    feed: shadow::Feed,
) -> shadow::Facts {
    shadow::Facts {
        asked_ms,
        live_fed,
        fed_since_ms,
        dropped_ms: None,
        barrier: true,
        feed,
    }
}

fn decide_on(
    cache: &MessageCache,
    chan: &ChanKey,
    ask: &shadow::Ask,
    shape: observe::Shape,
    via_flat: bool,
    facts: shadow::Facts,
) -> (shadow::Decision, shadow::View) {
    let query = shadow::query(ask, shape, via_flat).expect("a query");
    let view = cache.shadow_view(chan, &query).served().expect("served");
    (shadow::decide(ask, &view, &facts), view)
}

fn decide(
    fx: &Fixture,
    ask: &shadow::Ask,
    facts: shadow::Facts,
) -> (shadow::Decision, shadow::View) {
    decide_on(
        &fx.cache,
        &fx.chan,
        ask,
        observe::Shape::DiscordNative,
        false,
        facts,
    )
}

fn row_ids(view: &shadow::View) -> Vec<String> {
    view.rows.iter().map(|r| r.message.id.0.clone()).collect()
}

/// A cache in `dir` on `clock`, routed for `chan` alone and swept.
fn reopen(dir: &crate::testing::TempDir, clock: &Arc<ManualClock>, chan: &ChanKey) -> MessageCache {
    let cache = fixtures::open(dir, clock, Limits::default());
    cache.retain_only(RouteSnapshot::new(
        [chan.clone()],
        [chan.provider_key.clone()],
    ));
    assert!(cache.quiesce_blocking(WAIT));
    cache
}

fn offer_now(cache: &MessageCache, chan: &ChanKey, observation: Observation) {
    let stamp = cache.stamp(chan);
    cache.offer(chan.clone(), stamp, observation);
    assert!(cache.quiesce_blocking(WAIT));
}

fn flat_listing(rows: Vec<Message>, observed_ms: i64) -> Observation {
    Observation::Listing(Listing {
        scope: Scope::Flat,
        rows: rows
            .into_iter()
            .map(|m| Incoming::new(m, Scope::Flat))
            .collect(),
        observed_ms,
        proof: Some(newest_proof(ListStart::At(T0 + STEP))),
    })
}

/// A count of everything from the first message.
fn count_from_first() -> shadow::Ask {
    shadow::Ask::Count {
        since_ms: Some(T0 + STEP),
        cap: 100,
    }
}

// ---- the week ------------------------------------------------------------------------------------

/// A stretch re-read by a range read within the week keeps its rows and its claim; the stretch
/// beside it, read only once eight days ago, loses its rows, and loses its claim in the same pass.
#[test]
fn a_stretch_revalidated_only_in_part_keeps_its_claim_only_where_it_was_read_again() {
    let fx = Fixture::new("shadow-week-part");
    let first = T0 + 8 * STEP + 1_000;
    fx.clock.set(first);
    fx.list(
        (1..=8).map(m).collect(),
        first,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let day6 = first + 6 * DAY;
    fx.clock.set(day6);
    fx.list(
        vec![m(3), m(4)],
        day6,
        Some(Proof {
            from: ListStart::At(T0 + 3 * STEP),
            until: ListEnd::Before(T0 + 5 * STEP),
            complete: true,
            full: true,
            source: Source::Read,
        }),
    );
    let now = first + 8 * DAY;
    fx.clock.set(now);
    fx.cache.maintain(Maintenance::Hourly);
    fx.settle();
    let at = facts(now + 1, false, None, shadow::Feed::AdditionsOnly);
    let whole = shadow::Ask::Page {
        since_ms: T0 + STEP,
        until_ms: Some(T0 + 5 * STEP),
        limit: 20,
    };
    let (decision, view) = decide(&fx, &whole, at);
    assert_ne!(
        decision,
        shadow::Decision::Serve,
        "page [m1, m5) served with only {:?}; claims {:?}",
        row_ids(&view),
        fx.cache.coverage(&fx.chan, &Scope::Main)
    );
    let reread = shadow::Ask::Page {
        since_ms: T0 + 3 * STEP,
        until_ms: Some(T0 + 5 * STEP),
        limit: 20,
    };
    let (decision, view) = decide(&fx, &reread, at);
    assert_eq!(
        decision,
        shadow::Decision::Serve,
        "the re-read piece: {:?}",
        row_ids(&view)
    );
    assert_eq!(row_ids(&view), vec!["m3".to_owned(), "m4".to_owned()]);
}

// ---- what lies under a fresh head ----------------------------------------------------------------

/// A count over twelve rows read an hour ago, under a head made fresh by a read of the newest two:
/// stale, whether or not a poller feeds the channel (Discord's feed carries no edits or
/// deletions).
#[test]
fn a_count_never_rests_on_an_hour_old_claim_under_a_head_a_short_read_made_fresh() {
    let fx = Fixture::new("shadow-count-hour");
    let first = T0 + 12 * STEP + 1_000;
    fx.clock.set(first);
    fx.list(
        (1..=12).map(m).collect(),
        first,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let later = first + HOUR;
    fx.clock.set(later);
    fx.list(
        vec![m(11), m(12)],
        later,
        Some(newest_proof(ListStart::At(T0 + 11 * STEP))),
    );
    for live in [false, true] {
        let at = facts(
            later + 5_000,
            live,
            Some(fx.cache.fed_since_ms()),
            shadow::Feed::AdditionsOnly,
        );
        let (decision, view) = decide(&fx, &count_from_first(), at);
        assert_ne!(
            decision,
            shadow::Decision::Serve,
            "live_fed {live}: a count of {} rows served on an hour-old claim",
            view.rows.len()
        );
    }
}

// ---- a restart -----------------------------------------------------------------------------------

/// A live-fed bridge's message, seen before a 25-hour stop, is not vouched for by the feed after
/// the restart; the same message pushed again after the restart is.
#[test]
fn a_restart_after_a_day_lets_the_feed_vouch_only_for_what_it_carried_since() {
    let dir = crate::testing::TempDir::new("shadow-restart");
    let first = T0 + 8 * STEP + 1_000;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let chan = ChanKey::new("p", "C1");
    {
        let cache = reopen(&dir, &clock, &chan);
        offer_now(&cache, &chan, flat_listing((1..=8).map(m).collect(), first));
    }
    let later = first + 25 * HOUR;
    clock.set(later);
    let cache = reopen(&dir, &clock, &chan);
    let ask = shadow::Ask::Message {
        consumer: shadow::Consumer::MessageById,
        id: MessageId("m3".to_owned()),
    };
    let fed = |asked_ms: i64| {
        facts(
            asked_ms,
            true,
            Some(cache.fed_since_ms()),
            shadow::Feed::Deletions,
        )
    };
    let (decision, _) = decide_on(
        &cache,
        &chan,
        &ask,
        observe::Shape::Bridge,
        true,
        fed(later + 1_000),
    );
    assert_ne!(
        decision,
        shadow::Decision::Serve,
        "a message seen before a 25 h stop vouched for by a feed that did not run"
    );
    // Pushed again, as it is, after the restart: the feed has carried it since.
    clock.set(later + 2_000);
    offer_now(
        &cache,
        &chan,
        Observation::Push {
            push: Push::Upsert {
                message: Box::new(Incoming::new(m(3), Scope::Flat)),
                update: true,
                partial: false,
            },
            observed_ms: later + 2_000,
        },
    );
    let (decision, _) = decide_on(
        &cache,
        &chan,
        &ask,
        observe::Shape::Bridge,
        true,
        fed(later + 3_000),
    );
    assert_eq!(
        decision,
        shadow::Decision::Serve,
        "seen by the feed since the restart"
    );
}

// ---- the unsettled band ------------------------------------------------------------------------

/// Thread scope: a reply in the band that the latest read of the thread's newest end did not list
/// is not served from a newest page of the thread.
#[test]
fn a_thread_band_reply_the_latest_read_did_not_list_is_never_served() {
    let fx = Fixture::new("shadow-thread-band");
    let thread = Scope::Thread("t1".to_owned());
    let replies = || {
        (1..=6)
            .map(|i| reply(&format!("r{i}"), "t1", &format!("reply {i}"), T0 + i * STEP))
            .collect::<Vec<_>>()
    };
    let gone = reply("rx", "t1", "posted and deleted", T0 + 6 * STEP - 30_000);
    let first = T0 + 7 * STEP;
    fx.clock.set(first);
    let mut with = replies();
    with.push(gone);
    fx.list_scope(
        thread.clone(),
        with,
        first,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let second = first + 30_000;
    fx.clock.set(second);
    fx.list_scope(
        thread,
        replies(),
        second,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let ask = shadow::Ask::Timeline {
        view: crate::threads::TimelineView::Thread,
        thread: Some("t1".to_owned()),
        limit: 5,
    };
    let at = facts(second + 1_000, false, None, shadow::Feed::AdditionsOnly);
    let (decision, view) = decide(&fx, &ask, at);
    assert!(
        row_ids(&view).contains(&"rx".to_owned()),
        "{:?}",
        row_ids(&view)
    );
    assert_ne!(
        decision,
        shadow::Decision::Serve,
        "rows {:?}",
        row_ids(&view)
    );
}

/// The same band, asked as a count: a message deleted moments after it was posted, held from an
/// earlier read, and not listed by the latest read of the newest end, is never counted.
#[test]
fn a_count_never_counts_a_band_row_held_from_an_earlier_read_the_latest_did_not_list() {
    let fx = Fixture::new("shadow-count-band");
    let gone = message("bx", "ada", "posted and deleted", T0 + 6 * STEP - 30_000);
    let first = T0 + 6 * STEP + 1_000;
    fx.clock.set(first);
    let mut with: Vec<Message> = (1..=6).map(m).collect();
    with.push(gone);
    fx.list(with, first, Some(newest_proof(ListStart::At(T0 + STEP))));
    let second = first + 30_000;
    fx.clock.set(second);
    fx.list(
        (1..=6).map(m).collect(),
        second,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let at = facts(
        second + 1_000,
        true,
        Some(fx.cache.fed_since_ms()),
        shadow::Feed::AdditionsOnly,
    );
    let (decision, view) = decide(&fx, &count_from_first(), at);
    let ids = row_ids(&view);
    assert!(ids.contains(&"bx".to_owned()), "{ids:?}");
    assert_eq!(
        decision,
        shadow::Decision::Miss(shadow::Miss::Gap),
        "a count of {} rows, including bx, which the latest read did not list: {ids:?}",
        ids.len()
    );
}

/// No race at all: a poller pushes a just-posted message; it is deleted (Discord's feed carries no
/// deletions); the next read of the newest end lacks it. Neither a count nor a window holds it,
/// whether it was written inside the band or above the head.
#[test]
fn a_count_never_counts_a_pushed_row_the_latest_read_did_not_list_in_the_band_or_above_the_head() {
    let fx = Fixture::new("shadow-count-pushed");
    let first = T0 + 6 * STEP + 1_000;
    fx.clock.set(first);
    fx.list(
        (1..=6).map(m).collect(),
        first,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let mut at_ms = first;
    for (label, created) in [
        ("in the band", T0 + 6 * STEP - 30_000),
        ("above the head", T0 + 6 * STEP + 500),
    ] {
        let id = format!("p{created}");
        at_ms += 5_000;
        fx.clock.set(at_ms);
        fx.offer(
            &fx.chan,
            Observation::Push {
                push: Push::Upsert {
                    message: Box::new(Incoming::new(
                        message(&id, "ada", "pushed then deleted", created),
                        Scope::Main,
                    )),
                    update: false,
                    partial: false,
                },
                observed_ms: at_ms,
            },
        );
        at_ms += 20_000;
        fx.clock.set(at_ms);
        fx.list(
            (1..=6).map(m).collect(),
            at_ms,
            Some(newest_proof(ListStart::At(T0 + STEP))),
        );
        let at = facts(
            at_ms + 1_000,
            true,
            Some(fx.cache.fed_since_ms()),
            shadow::Feed::AdditionsOnly,
        );
        let (decision, view) = decide(&fx, &count_from_first(), at);
        let ids = row_ids(&view);
        assert!(ids.contains(&id), "{label}: {ids:?}");
        assert_eq!(
            decision,
            shadow::Decision::Miss(shadow::Miss::Gap),
            "{label}: a count of {} rows including {id}, which the latest read did not list: \
             {ids:?}",
            ids.len()
        );
        let window = shadow::Ask::Window {
            consumer: shadow::Consumer::Messages,
            limit: 3,
        };
        let (decision, view) = decide(&fx, &window, at);
        assert_ne!(
            decision,
            shadow::Decision::Serve,
            "{label}: window {:?}",
            row_ids(&view)
        );
    }
}

// ---- a write that fails --------------------------------------------------------------------------

/// A live-fed bridge. A pushed DELETION's write fails (the file locked past the busy timeout). The
/// message is still held; the feed that carries deletions does not vouch for it, now or after a
/// restart.
#[test]
fn a_pushed_deletion_whose_write_failed_is_never_vouched_for_even_after_a_restart() {
    let dir = crate::testing::TempDir::new("shadow-failed-deletion");
    let first = T0 + 8 * STEP + 1_000;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let chan = ChanKey::new("p", "C1");
    let ask = shadow::Ask::Message {
        consumer: shadow::Consumer::MessageById,
        id: MessageId("m3".to_owned()),
    };
    {
        let cache = reopen(&dir, &clock, &chan);
        offer_now(&cache, &chan, flat_listing((1..=8).map(m).collect(), first));
        let (release, locker) = super::tests::hold_write_lock(&cache);
        clock.set(first + 10_000);
        let stamp = cache.stamp(&chan);
        cache.offer(
            chan.clone(),
            stamp,
            Observation::Push {
                push: Push::Delete {
                    id: MessageId("m3".to_owned()),
                    created_ms: None,
                },
                observed_ms: first + 10_000,
            },
        );
        let started = std::time::Instant::now();
        while cache.stats().ops_failed == 0 {
            assert!(started.elapsed() < WAIT, "the write never failed");
            std::thread::sleep(Duration::from_millis(10));
        }
        release.send(()).expect("release");
        locker.join().expect("locker");
        assert!(cache.quiesce_blocking(WAIT));
        assert!(
            cache
                .message(&chan, &MessageId("m3".to_owned()))
                .served()
                .expect("served")
                .is_some(),
            "the deletion never reached the file"
        );
        clock.set(first + 20_000);
        let mut fed = facts(
            first + 20_000,
            true,
            Some(cache.fed_since_ms()),
            shadow::Feed::Deletions,
        );
        fed.dropped_ms = cache.last_drop_ms(&chan);
        let (decision, _) = decide_on(&cache, &chan, &ask, observe::Shape::Bridge, true, fed);
        assert_ne!(
            decision,
            shadow::Decision::Serve,
            "the deleted message vouched for"
        );
    }
    // An hour later (inside the bridge's day), after a restart: what the file says still holds.
    clock.set(first + HOUR);
    let cache = reopen(&dir, &clock, &chan);
    let fed = facts(
        first + HOUR + 1_000,
        true,
        Some(cache.fed_since_ms()),
        shadow::Feed::Deletions,
    );
    let (decision, _) = decide_on(&cache, &chan, &ask, observe::Shape::Bridge, true, fed);
    assert_ne!(
        decision,
        shadow::Decision::Serve,
        "after a restart the deleted message is vouched for"
    );
}

// ---- what a sighting does not prove --------------------------------------------------------------

fn main_page(limit: usize) -> shadow::Ask {
    shadow::Ask::Timeline {
        view: crate::threads::TimelineView::Main,
        thread: None,
        limit,
    }
}

fn root(id: &str, text: &str, created_ms: i64) -> Message {
    let mut root = message(id, "ada", text, created_ms);
    root.thread = Some(crate::threads::MessageThread {
        id: id.to_owned(),
        root_message_id: Some(MessageId(id.to_owned())),
        is_root: true,
        reply_count: Some(1),
        reply_count_exact: true,
    });
    root
}

/// Slack. m1..m6 are listed; ten minutes on u, w and x are written; five minutes after that a read
/// of the newest end lists only its newest two, w and x. Then v, a root written before u, is read
/// by id (as a thread's root is). v exists, but nothing says u does not: upstream's newest three are
/// u, w and x, and u was written six minutes before that read began, so late visibility does not
/// explain its absence. Neither a window nor, an hour on, a newest page is served.
#[test]
fn a_band_row_read_by_id_after_a_short_read_of_the_newest_end_never_hides_the_message_beside_it() {
    let fx = Fixture::new("shadow-band-by-id");
    let m6 = T0 + 6 * STEP;
    let first = m6 + 1_000;
    fx.clock.set(first);
    fx.list(
        (1..=6).map(m).collect(),
        first,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let x_ms = m6 + 10 * 60_000;
    let v = root("v", "a root with replies", x_ms - 100_000);
    let w = message("w", "ada", "w", x_ms - 20_000);
    let x = message("x", "ada", "x", x_ms);
    // u, at x - 60 s, is never held.
    let second = x_ms + 5 * 60_000;
    fx.clock.set(second);
    fx.list(
        vec![w, x],
        second,
        Some(newest_proof(ListStart::At(x_ms - 20_000))),
    );
    let third = second + 2_000;
    fx.clock.set(third);
    for observation in observe::single(observe::Shape::Slack, &v, "v", third) {
        fx.offer(&fx.chan, observation);
    }
    let digest = shadow::Ask::Window {
        consumer: shadow::Consumer::Digest,
        limit: 3,
    };
    for (ask, asked_ms) in [(digest, third + 1_000), (main_page(3), third + HOUR)] {
        let (decision, view) = decide_on(
            &fx.cache,
            &fx.chan,
            &ask,
            observe::Shape::Slack,
            false,
            facts(asked_ms, false, None, shadow::Feed::AdditionsOnly),
        );
        assert_eq!(row_ids(&view), ["v", "w", "x"], "{ask:?}");
        assert_eq!(
            decision,
            shadow::Decision::Miss(shadow::Miss::Gap),
            "{ask:?} served without u"
        );
    }
}

/// Discord. The same with rows only, a short page's worth, read after a read of the newest end
/// that listed two rows: what that read did not list is vouched for by nothing.
#[test]
fn a_band_row_a_read_of_rows_saw_after_the_latest_listing_never_closes_a_gap() {
    let fx = Fixture::new("shadow-band-rows");
    let m6 = T0 + 6 * STEP;
    let first = m6 + 1_000;
    fx.clock.set(first);
    fx.list(
        (1..=6).map(m).collect(),
        first,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let v = message("v", "ada", "v", m6 + 20_000);
    // u, at m6 + 40 s, is never held.
    let w = message("w", "ada", "w", m6 + 60_000);
    let x = message("x", "ada", "x", m6 + 80_000);
    let second = m6 + 90_000;
    fx.clock.set(second);
    fx.list(
        vec![w, x],
        second,
        Some(newest_proof(ListStart::At(m6 + 60_000))),
    );
    let third = second + 2_000;
    fx.clock.set(third);
    fx.offer(
        &fx.chan,
        Observation::Rows {
            rows: vec![Incoming::new(v, Scope::Main)],
            scope: Some(Scope::Main),
            observed_ms: third,
        },
    );
    let window = shadow::Ask::Window {
        consumer: shadow::Consumer::Messages,
        limit: 3,
    };
    for (ask, asked_ms) in [(window, third + 1_000), (main_page(3), third + HOUR)] {
        let (decision, view) = decide(
            &fx,
            &ask,
            facts(asked_ms, false, None, shadow::Feed::AdditionsOnly),
        );
        assert_eq!(row_ids(&view), ["v", "w", "x"], "{ask:?}");
        assert_eq!(
            decision,
            shadow::Decision::Miss(shadow::Miss::Gap),
            "{ask:?} served without u"
        );
    }
}

/// A message deleted moments after it was posted lies in the band, where its absence from the
/// latest read of the newest end proves nothing, so the file still holds it. A push of it that
/// lands after that read (an event retried, a poller's publish of what it read before) says only
/// that it existed once: it never vouches for the row, in a window or a count.
#[test]
fn a_late_push_of_a_band_row_the_latest_read_did_not_list_never_vouches_for_it() {
    let fx = Fixture::new("shadow-band-late-push");
    let gone = message("bx", "ada", "posted and deleted", T0 + 6 * STEP - 30_000);
    let first = T0 + 6 * STEP + 1_000;
    fx.clock.set(first);
    let mut with: Vec<Message> = (1..=6).map(m).collect();
    with.push(gone.clone());
    fx.list(with, first, Some(newest_proof(ListStart::At(T0 + STEP))));
    let second = first + 5_000;
    fx.clock.set(second);
    fx.list(
        (1..=6).map(m).collect(),
        second,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let third = second + 2_000;
    fx.clock.set(third);
    fx.offer(
        &fx.chan,
        Observation::Push {
            push: Push::Upsert {
                message: Box::new(Incoming::new(gone, Scope::Main)),
                update: false,
                partial: false,
            },
            observed_ms: third,
        },
    );
    let at = facts(third + 1_000, false, None, shadow::Feed::AdditionsOnly);
    let window = shadow::Ask::Window {
        consumer: shadow::Consumer::Messages,
        limit: 3,
    };
    for ask in [window, count_from_first()] {
        let (decision, view) = decide(&fx, &ask, at);
        assert!(row_ids(&view).contains(&"bx".to_owned()), "{ask:?}");
        assert_eq!(
            decision,
            shadow::Decision::Miss(shadow::Miss::Gap),
            "{ask:?} holds bx, deleted before the latest read: {:?}",
            row_ids(&view)
        );
    }
}

/// This server's own post, an hour after the latest read of the newest end, with messages written
/// in between that no read listed: a newest page answered from the cache would hold a gap of an
/// hour beneath the post, so none is.
#[test]
fn an_own_post_above_a_stale_head_never_makes_a_newest_page_with_a_gap() {
    let fx = Fixture::new("shadow-own-post");
    let m6 = T0 + 6 * STEP;
    let first = m6 + 1_000;
    fx.clock.set(first);
    fx.list(
        (1..=6).map(m).collect(),
        first,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let posted_at = first + HOUR;
    fx.clock.set(posted_at);
    let own = message("own", "me", "our reply", m6 + HOUR);
    for observation in observe::posted(observe::Shape::DiscordNative, &own, None, posted_at) {
        fx.offer(&fx.chan, observation);
    }
    let (decision, view) = decide(
        &fx,
        &main_page(3),
        facts(posted_at + 5_000, false, None, shadow::Feed::AdditionsOnly),
    );
    assert_eq!(row_ids(&view), ["m5", "m6", "own"]);
    assert_eq!(decision, shadow::Decision::Miss(shadow::Miss::Gap));
}

/// A Slack Main listing carrying r1, a reply also sent to the channel (its home is its thread).
fn main_with_broadcast(fx: &Fixture, broadcast_ms: i64, observed_ms: i64) {
    let mut rows = vec![Incoming::new(root("m1", "body 1", T0 + STEP), Scope::Main)];
    rows.extend((2..=6).map(|n| Incoming::new(m(n), Scope::Main)));
    rows.push(Incoming::new(
        reply("r1", "m1", "also in the channel", broadcast_ms),
        Scope::Thread("m1".to_owned()),
    ));
    fx.offer(
        &fx.chan,
        Observation::Listing(Listing {
            scope: Scope::Main,
            rows,
            observed_ms,
            proof: Some(newest_proof(ListStart::At(T0 + STEP))),
        }),
    );
}

/// Slack. An earlier read of Main listed r1, a reply also sent to the channel, in Main's band; the
/// latest read of Main's newest end did not. A read of r1's thread since lists it: that read
/// vouches for r1 in its thread, never in Main, so a window of Main holding r1 is not served.
#[test]
fn a_read_of_one_scope_never_vouches_for_a_row_in_anothers_band() {
    let fx = Fixture::new("shadow-band-other-scope");
    let first = T0 + 6 * STEP + 1_000;
    fx.clock.set(first);
    let broadcast_ms = T0 + 6 * STEP - 30_000;
    main_with_broadcast(&fx, broadcast_ms, first);
    let second = first + 5_000;
    fx.clock.set(second);
    let mut main = vec![Incoming::new(root("m1", "body 1", T0 + STEP), Scope::Main)];
    main.extend((2..=6).map(|n| Incoming::new(m(n), Scope::Main)));
    fx.offer(
        &fx.chan,
        Observation::Listing(Listing {
            scope: Scope::Main,
            rows: main,
            observed_ms: second,
            proof: Some(newest_proof(ListStart::At(T0 + STEP))),
        }),
    );
    let third = second + 2_000;
    fx.clock.set(third);
    let thread = Scope::Thread("m1".to_owned());
    fx.offer(
        &fx.chan,
        Observation::Listing(Listing {
            scope: thread.clone(),
            rows: vec![
                Incoming::new(root("m1", "body 1", T0 + STEP), Scope::Main),
                Incoming::new(
                    reply("r1", "m1", "also in the channel", broadcast_ms),
                    thread.clone(),
                ),
            ],
            observed_ms: third,
            proof: Some(newest_proof(ListStart::At(T0 + STEP))),
        }),
    );
    let window = shadow::Ask::Window {
        consumer: shadow::Consumer::Messages,
        limit: 3,
    };
    let (decision, view) = decide_on(
        &fx.cache,
        &fx.chan,
        &window,
        observe::Shape::Slack,
        false,
        facts(third + 1_000, false, None, shadow::Feed::AdditionsOnly),
    );
    assert_eq!(row_ids(&view), ["m5", "r1", "m6"]);
    assert_eq!(
        decision,
        shadow::Decision::Miss(shadow::Miss::Gap),
        "r1 vouched for in Main by a read of its thread"
    );
    // The same read of the thread does vouch for it there.
    let page = shadow::Ask::Timeline {
        view: crate::threads::TimelineView::Thread,
        thread: Some("m1".to_owned()),
        limit: 2,
    };
    let (decision, view) = decide_on(
        &fx.cache,
        &fx.chan,
        &page,
        observe::Shape::Slack,
        false,
        facts(third + 1_000, false, None, shadow::Feed::AdditionsOnly),
    );
    assert_eq!(row_ids(&view), ["m1", "r1"]);
    assert_eq!(
        decision,
        shadow::Decision::Serve,
        "the thread's own read vouches for its band"
    );
}
