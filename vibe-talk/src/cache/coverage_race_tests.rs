//! The coverage races `#297 cache-coverage-races` closes, each kept as the probe that showed it:
//! the reviews of `#208 persistent-cache` milestone `cache-fill` wrote them, and they are named as
//! the reviews named them.
//!
//! - **R6-A**: an erase of one scope (a thread whose access is lost) racing a read of another
//!   scope that lists one of the erased messages (a Slack reply also sent to the channel). The
//!   read's stamp is current, yet the erase's tombstone holds the message back; the read must not
//!   claim the instant it carried the message at.
//! - **R6-C**: the same erase must not discard a pushed deletion or edit of a message it kept, nor
//!   leave the feed vouching for the row that change would have replaced.
//! - **R5-A**: a window asked moments after a read of the newest end is not stale.
//!
//! And what the reviews of the fix itself found:
//!
//! - **An adapter's edit with no edit time**: the edit times that keep an older copy out must not
//!   throw away an edit an adapter pushes, for any provider, with no edit time.
//! - **A refused sweep**: a sweep the file refused has dropped what was on its way for the
//!   channels it left out, so it must erase them when it is carried out, even one put back on the
//!   list since; and an erase carried out late must not let a read begun before it land after it.

use std::sync::atomic::AtomicI64;
use std::sync::Arc;
use std::time::Duration;

use super::fixtures::{self, message, newest_proof, reply, Fixture, ManualClock, T0};
use super::tests::{m, STEP};
use super::*;
use crate::model::Message;

const SEC: i64 = 1_000;
const HOUR: i64 = 3_600_000;
const WAIT: Duration = Duration::from_secs(30);

/// What a shadow evaluation knows beside the snapshot, at `asked_ms`: no live feed, no drop.
fn facts(asked_ms: i64) -> shadow::Facts {
    shadow::Facts {
        asked_ms,
        live_fed: false,
        fed_since_ms: None,
        dropped_ms: None,
        barrier: true,
        feed: shadow::Feed::AdditionsOnly,
    }
}

/// What the cache would decide for `ask` of `fx`'s channel, read as `shape` reads it, and the ids
/// of the rows it would hand back.
fn decide_shape(
    fx: &Fixture,
    ask: &shadow::Ask,
    shape: observe::Shape,
    at: shadow::Facts,
) -> (shadow::Decision, Vec<String>) {
    let query = shadow::query(ask, shape, false).expect("a query");
    let view = fx
        .cache
        .shadow_view(&fx.chan, &query)
        .served()
        .expect("served");
    let ids = view.rows.iter().map(|r| r.message.id.0.clone()).collect();
    (shadow::decide(ask, &view, &at), ids)
}

/// A root with `replies` replies, its thread named after it.
fn root(id: &str, created_ms: i64, replies: u64) -> Message {
    let mut root = message(id, "ada", "root", created_ms);
    root.thread = Some(crate::threads::MessageThread {
        id: id.to_owned(),
        root_message_id: Some(MessageId(id.to_owned())),
        is_root: true,
        reply_count: Some(replies),
        reply_count_exact: true,
    });
    root
}

/// A proven newest read of Main: m1 (the root of thread m1), m2 to m6, and r1, a reply in thread
/// m1 also sent to the channel, created at `broadcast_ms`.
fn main_listing(broadcast_ms: i64, observed_ms: i64) -> Observation {
    let mut rows = vec![Incoming::new(root("m1", T0 + STEP, 1), Scope::Main)];
    rows.extend((2..=6).map(|n| Incoming::new(m(n), Scope::Main)));
    rows.push(Incoming::new(
        reply("r1", "m1", "also in the channel", broadcast_ms),
        Scope::Thread("m1".to_owned()),
    ));
    Observation::Listing(Listing {
        scope: Scope::Main,
        rows,
        observed_ms,
        proof: Some(newest_proof(ListStart::At(T0 + STEP))),
    })
}

fn window(limit: usize) -> shadow::Ask {
    shadow::Ask::Window {
        consumer: shadow::Consumer::Messages,
        limit,
    }
}

fn fallback(limit: usize) -> shadow::Ask {
    shadow::Ask::Timeline {
        view: crate::threads::TimelineView::Main,
        thread: None,
        limit,
    }
}

/// R6-A, Slack. r1 is a reply also sent to the channel. Its thread is erased (access confirmed
/// lost). A read of Main that began after the erase was asked for (its stamp is current), but
/// before the writer applied the erase (the writer behind, or in the same millisecond), lists r1
/// and lands after the erase. r1 is held back by the erase's tombstone (the read did not begin
/// after it), so the read's claim over Main must not cover r1's instant again: below the band
/// that would undo the erase's puncture of Main's coverage, and in it, being the latest read of the
/// newest end, it would write back down where its listing began. A window three seconds later and
/// a fallback page an hour later would then be served without a message upstream still lists.
#[test]
fn r6_a_a_main_read_begun_before_a_thread_erase_was_applied_never_reclaims_the_reply() {
    let mut failures = Vec::new();
    for (label, broadcast_ms, lag) in [
        ("below the band, writer 20 ms behind", T0 + 3 * STEP + 5, 20),
        ("below the band, same millisecond", T0 + 3 * STEP + 5, 0),
        (
            "in the band, writer 20 ms behind",
            T0 + 6 * STEP - 30_000,
            20,
        ),
    ] {
        let fx = Fixture::new("r6-a");
        let first = T0 + 6 * STEP + 1_000;
        fx.clock.set(first);
        fx.offer(&fx.chan, main_listing(broadcast_ms, first));
        let erase_at = first + SEC;
        fx.clock.set(erase_at);
        // The erase is asked for: the epoch moves, the op is queued.
        fx.cache
            .erase_scope(&fx.chan, &Scope::Thread("m1".to_owned()));
        // A read of Main starts now, after the epoch moved: its stamp is current.
        let stamp = fx.cache.stamp(&fx.chan);
        let read_began = erase_at;
        // The writer applies the erase `lag` ms later.
        fx.clock.set(erase_at + lag);
        fx.settle();
        // The read's answer lands.
        fx.cache.offer(
            fx.chan.clone(),
            stamp,
            main_listing(broadcast_ms, read_began),
        );
        fx.settle();
        let held = fx.ids(&Scope::Main);
        let claims = fx
            .cache
            .coverage(&fx.chan, &Scope::Main)
            .served()
            .expect("served");
        if !held.contains(&"r1".to_owned())
            && coverage::covers(&claims, broadcast_ms, broadcast_ms + 1).is_some()
        {
            failures.push(format!(
                "{label}: Main claims r1's instant without r1: {claims:?}"
            ));
        }
        let (d, ids) = decide_shape(
            &fx,
            &window(4),
            observe::Shape::Slack,
            facts(read_began + lag + 3 * SEC),
        );
        let (d2, ids2) = decide_shape(
            &fx,
            &fallback(4),
            observe::Shape::Slack,
            facts(read_began + HOUR),
        );
        let r1_held = held.contains(&"r1".to_owned());
        if !(r1_held || (d != shadow::Decision::Serve && d2 != shadow::Decision::Serve)) {
            failures.push(format!(
                "{label}: window {d:?} {ids:?} / fallback {d2:?} {ids2:?} served without r1"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// R6-B, Discord's own API (complete coverage). As R6-A, for a closed page across r1's instant:
/// never served without r1, however late it is asked.
#[test]
fn r6_b_a_page_across_the_reclaimed_instant_is_never_served() {
    let fx = Fixture::new("r6-b");
    let first = T0 + 6 * STEP + 1_000;
    let broadcast_ms = T0 + 3 * STEP + 5;
    fx.clock.set(first);
    fx.offer(&fx.chan, main_listing(broadcast_ms, first));
    let erase_at = first + SEC;
    fx.clock.set(erase_at);
    fx.cache
        .erase_scope(&fx.chan, &Scope::Thread("m1".to_owned()));
    let stamp = fx.cache.stamp(&fx.chan);
    fx.clock.set(erase_at + 20);
    fx.settle();
    fx.cache
        .offer(fx.chan.clone(), stamp, main_listing(broadcast_ms, erase_at));
    fx.settle();
    let page = shadow::Ask::Page {
        since_ms: T0 + 3 * STEP,
        until_ms: Some(T0 + 4 * STEP + 1),
        limit: 10,
    };
    let (d, ids) = decide_shape(
        &fx,
        &page,
        observe::Shape::DiscordNative,
        facts(erase_at + HOUR),
    );
    assert_ne!(
        d,
        shadow::Decision::Serve,
        "page {ids:?} served without r1 (upstream: m3, r1, m4)"
    );
}

/// RV-1: a read of Main that begins after the thread's erase was asked for (its stamp is current)
/// but before the writer applied it lands after the erase, its read begun 50 ms before the writer
/// applied it. r1 is held out by the erase's tombstone, and the read neither re-stamps Main's claim
/// over r1's instant nor re-records where its listing began below it.
#[test]
fn rv1_a_main_read_begun_while_the_erase_was_queued_never_reclaims_r1s_instant() {
    let mut failures = Vec::new();
    for (label, broadcast_ms) in [
        ("below the band", T0 + 3 * STEP + 5),
        ("in the band", T0 + 6 * STEP - 30_000),
    ] {
        let fx = Fixture::new("rv1");
        let first = T0 + 6 * STEP + 1_000;
        fx.clock.set(first);
        fx.offer(&fx.chan, main_listing(broadcast_ms, first));
        assert!(fx.ids(&Scope::Main).contains(&"r1".to_owned()));
        let erase_at = first + 1_000;
        fx.clock.set(erase_at);
        fx.cache
            .erase_scope(&fx.chan, &Scope::Thread("m1".to_owned()));
        // A read of Main starts now: after the erase was asked for, so its stamp is current.
        let stamp = fx.cache.stamp(&fx.chan);
        let read_began = fx.cache.now_ms();
        fx.clock.set(erase_at + 50);
        fx.settle();
        fx.cache.offer(
            fx.chan.clone(),
            stamp,
            main_listing(broadcast_ms, read_began),
        );
        fx.settle();
        for (ask, asked) in [
            (window(4), read_began + 3_000),
            (fallback(4), read_began + HOUR),
        ] {
            let (d, rows) = decide_shape(&fx, &ask, observe::Shape::Slack, facts(asked));
            if d == shadow::Decision::Serve && !rows.contains(&"r1".to_owned()) {
                failures.push(format!("{label}: {ask:?} served {rows:?} without r1"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// RV-1b: as RV-1, in strict order, the writer held still: the erase is asked for at X, a read of
/// Main begins at X + 10 (its stamp current), and the writer applies the erase at X + 20. The read
/// lands after.
#[test]
fn rv1b_strictly_between_the_erase_call_and_its_apply() {
    let mut failures = Vec::new();
    for (label, broadcast_ms) in [
        ("below the band", T0 + 3 * STEP + 5),
        ("in the band", T0 + 6 * STEP - 30_000),
    ] {
        let fx = Fixture::new("rv1b");
        let first = T0 + 6 * STEP + 1_000;
        fx.clock.set(first);
        fx.offer(&fx.chan, main_listing(broadcast_ms, first));
        let x = first + 1_000;
        let resume = fx.cache.pause_writer();
        fx.clock.set(x);
        fx.cache
            .erase_scope(&fx.chan, &Scope::Thread("m1".to_owned()));
        fx.clock.set(x + 10);
        let stamp = fx.cache.stamp(&fx.chan);
        let read_began = fx.cache.now_ms();
        fx.clock.set(x + 20);
        drop(resume);
        fx.settle();
        fx.cache.offer(
            fx.chan.clone(),
            stamp,
            main_listing(broadcast_ms, read_began),
        );
        fx.settle();
        for (ask, asked) in [
            (window(4), read_began + 3_000),
            (fallback(4), read_began + HOUR),
        ] {
            let (d, rows) = decide_shape(&fx, &ask, observe::Shape::Slack, facts(asked));
            if d == shadow::Decision::Serve && !rows.contains(&"r1".to_owned()) {
                failures.push(format!("{label}: {ask:?} served {rows:?} without r1"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// R6-A, for the erased thread itself: a read of the thread begun after its erase was asked for,
/// but before the writer applied it, lands after it and lists the replies the erase's tombstones
/// hold back. It must not rebuild the thread's coverage over them, nor its head's vouch: a newest
/// page of the thread would then be served without the replies upstream lists.
#[test]
fn r6_a_a_thread_read_begun_before_its_erase_was_applied_never_rebuilds_its_coverage() {
    let fx = Fixture::new("r6-a-thread");
    let thread = Scope::Thread("m1".to_owned());
    let root_ms = T0 + STEP;
    let r1_ms = root_ms + 10 * SEC;
    let r2_ms = root_ms + 5 * 60 * SEC;
    let listing = |observed_ms: i64| {
        Observation::Listing(Listing {
            scope: thread.clone(),
            rows: vec![
                Incoming::new(root("m1", root_ms, 2), Scope::Main),
                Incoming::new(reply("r1", "m1", "one", r1_ms), thread.clone()),
                Incoming::new(reply("r2", "m1", "two", r2_ms), thread.clone()),
            ],
            observed_ms,
            proof: Some(newest_proof(ListStart::At(root_ms))),
        })
    };
    let first = T0 + 2 * STEP;
    fx.clock.set(first);
    fx.offer(&fx.chan, listing(first));
    assert_eq!(fx.ids(&thread), ["m1", "r1", "r2"]);
    let x = first + 60 * SEC;
    let resume = fx.cache.pause_writer();
    fx.clock.set(x);
    fx.cache.erase_scope(&fx.chan, &thread);
    fx.clock.set(x + 10);
    let stamp = fx.cache.stamp(&fx.chan);
    let read_began = fx.cache.now_ms();
    fx.clock.set(x + 20);
    drop(resume);
    fx.settle();
    fx.cache.offer(fx.chan.clone(), stamp, listing(read_began));
    fx.settle();
    let claims = fx
        .cache
        .coverage(&fx.chan, &thread)
        .served()
        .expect("served");
    assert!(
        coverage::covers(&claims, r1_ms, r1_ms + 1).is_none(),
        "the thread claims r1's instant without r1: {claims:?}"
    );
    let newest = shadow::Ask::Timeline {
        view: crate::threads::TimelineView::Thread,
        thread: Some("m1".to_owned()),
        limit: 1,
    };
    let (d, ids) = decide_shape(
        &fx,
        &newest,
        observe::Shape::DiscordNative,
        facts(read_began + 3 * SEC),
    );
    assert_ne!(
        d,
        shadow::Decision::Serve,
        "the thread's newest page {ids:?} served without r2 (upstream's newest: r2)"
    );
}

/// An open cache for channel `C1` of provider `p`, with a proven newest read of its whole Flat
/// list, m1 to m8, at `first`.
fn bridge_with_flat(
    dir: &crate::testing::TempDir,
    clock: &Arc<ManualClock>,
    first: i64,
) -> (MessageCache, ChanKey) {
    let chan = ChanKey::new("p", "C1");
    let cache = fixtures::open(dir, clock, Limits::default());
    cache.retain_only(RouteSnapshot::new(
        [chan.clone()],
        [chan.provider_key.clone()],
    ));
    assert!(cache.quiesce_blocking(WAIT));
    let flat = Observation::Listing(Listing {
        scope: Scope::Flat,
        rows: (1..=8).map(|n| Incoming::new(m(n), Scope::Flat)).collect(),
        observed_ms: first,
        proof: Some(newest_proof(ListStart::At(T0 + STEP))),
    });
    let stamp = cache.stamp(&chan);
    cache.offer(chan.clone(), stamp, flat);
    assert!(cache.quiesce_blocking(WAIT));
    (cache, chan)
}

/// What the cache would decide for m3 by id on a live-fed bridge, whose feed carries deletions,
/// asked at `asked_ms`, and the text it would serve.
fn decide_m3(
    cache: &MessageCache,
    chan: &ChanKey,
    asked_ms: i64,
) -> (shadow::Decision, Option<String>) {
    let ask = shadow::Ask::Message {
        consumer: shadow::Consumer::MessageById,
        id: MessageId("m3".to_owned()),
    };
    let query = shadow::query(&ask, observe::Shape::Bridge, true).expect("a query");
    let view = cache.shadow_view(chan, &query).served().expect("served");
    let facts = shadow::Facts {
        asked_ms,
        live_fed: true,
        fed_since_ms: Some(cache.fed_since_ms()),
        dropped_ms: cache.last_drop_ms(chan),
        barrier: true,
        feed: shadow::Feed::Deletions,
    };
    let content = view.rows.first().map(|r| r.message.content.clone());
    (shadow::decide(&ask, &view, &facts), content)
}

/// R6-C: a live-fed bridge. A pushed deletion of m3 is stamped just before the server erases one
/// thread of the channel (access to it confirmed lost), and queued after it. The erase must not
/// make the writer discard the deletion: the feed, which carries deletions, would then vouch for
/// the deleted message, with no drop recorded.
#[test]
fn r6_c_a_thread_erase_never_discards_a_queued_deletion_of_another_scopes_message() {
    let dir = crate::testing::TempDir::new("r6-c");
    let first = T0 + 8 * STEP + 1_000;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    clock.set(first + 10_000);
    // The deletion is received and stamped...
    let stamp = cache.stamp(&chan);
    // ...and before it is queued, a thread of the channel is erased.
    cache.erase_scope(&chan, &Scope::Thread("t9".to_owned()));
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
    assert!(cache.quiesce_blocking(WAIT));
    clock.set(first + 20_000);
    let (decision, content) = decide_m3(&cache, &chan, first + 20_000);
    assert_eq!(content, None, "the deletion was applied");
    assert_ne!(
        decision,
        shadow::Decision::Serve,
        "the deleted m3 vouched for by the feed"
    );
}

/// A pushed change of m3 queued, the writer held still, and then, while it waits, a thread the
/// cache never held erased: a read-scope request named a bogus thread, answered `thread_not_found`,
/// and the server's own read confirmed it ten minutes later. What the cache would then answer for
/// m3 by id, the text it holds, and how many observations were dropped as stale.
fn bogus_thread_erase(
    tag: &str,
    erase_bogus_thread: bool,
    edit: bool,
) -> (shadow::Decision, Option<String>, u64) {
    let dir = crate::testing::TempDir::new(tag);
    let first = T0 + 8 * STEP + 1_000;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    let at = first + 10_000;
    clock.set(at);
    let resume = cache.pause_writer();
    let push = if edit {
        let mut changed = m(3);
        changed.content = "EDITED".to_owned();
        Push::Upsert {
            message: Box::new(Incoming::new(changed, Scope::Flat)),
            update: true,
            partial: false,
        }
    } else {
        Push::Delete {
            id: MessageId("m3".to_owned()),
            created_ms: None,
        }
    };
    let stamp = cache.stamp(&chan);
    cache.offer(
        chan.clone(),
        stamp,
        Observation::Push {
            push,
            observed_ms: at,
        },
    );
    if erase_bogus_thread {
        cache.erase_scope(&chan, &Scope::Thread("1700000000.999999".to_owned()));
    }
    drop(resume);
    assert!(cache.quiesce_blocking(WAIT));
    let stale = cache.stats().ops_stale;
    clock.set(at + 5_000);
    let (decision, content) = decide_m3(&cache, &chan, at + 5_000);
    (decision, content, stale)
}

/// The control: with no erase, the pushed deletion lands, and m3 is not served.
#[test]
fn r6_control_a_pushed_deletion_lands() {
    let (decision, content, _) = bogus_thread_erase("r6-c-control", false, false);
    assert_eq!(content, None);
    assert_ne!(decision, shadow::Decision::Serve);
}

/// An erase of a thread the cache never held must not drop a queued deletion of a Flat message as
/// stale, the channel left unsuspected: the feed would then vouch for the deleted message.
#[test]
fn r6_a_bogus_thread_erase_never_lets_the_feed_vouch_for_a_deleted_message() {
    let (decision, content, stale) = bogus_thread_erase("r6-c-deleted", true, false);
    assert!(
        !(decision == shadow::Decision::Serve && content.is_some()),
        "a message deleted upstream would be served (stale ops {stale}, content {content:?})"
    );
}

/// The same for an edit: the old text must never be served.
#[test]
fn r6_a_bogus_thread_erase_never_lets_the_feed_vouch_for_an_edited_messages_old_text() {
    let (decision, content, stale) = bogus_thread_erase("r6-c-edited", true, true);
    assert!(
        !(decision == shadow::Decision::Serve && content.as_deref() != Some("EDITED")),
        "an edited message's old text would be served (stale ops {stale}, content {content:?})"
    );
}

/// R5-A: a quiet channel read every eight seconds. The band stays open (no newer row, under ten
/// minutes), so the head keeps the time of the read that opened it; a window asked two seconds
/// after the latest read of the newest end is judged by that read, and is fresh.
#[test]
fn r5_a_a_window_two_seconds_after_a_read_of_the_newest_end_is_not_stale() {
    let fx = Fixture::new("r5-a");
    let first = T0 + 6 * STEP + 1_000;
    fx.clock.set(first);
    let proof = || Some(newest_proof(ListStart::At(T0 + STEP)));
    fx.list((1..=6).map(m).collect(), first, proof());
    let mut at = first;
    for _ in 0..4 {
        at += 8 * SEC;
        fx.clock.set(at);
        fx.list((1..=6).map(m).collect(), at, proof());
    }
    let head = fx
        .cache
        .head(&fx.chan, &Scope::Main)
        .served()
        .expect("served")
        .expect("head");
    assert_eq!(head.head_listed_ms, first, "the band keeps its clock");
    let (d, ids) = decide_shape(
        &fx,
        &window(3),
        observe::Shape::DiscordNative,
        facts(at + 2 * SEC),
    );
    assert_eq!(
        d,
        shadow::Decision::Serve,
        "a window 2 s after a read of the newest end judged {d:?} ({ids:?})"
    );
    // Fifteen seconds after that read it is fresh still, and past them stale.
    let (d, _) = decide_shape(
        &fx,
        &window(3),
        observe::Shape::DiscordNative,
        facts(at + shadow::FRESH_UNFED_MS),
    );
    assert_eq!(d, shadow::Decision::Serve);
    let (d, _) = decide_shape(
        &fx,
        &window(3),
        observe::Shape::DiscordNative,
        facts(at + shadow::FRESH_UNFED_MS + 1),
    );
    assert_eq!(d, shadow::Decision::Miss(shadow::Miss::StaleHead));
}

/// An erase the file refuses (another connection holds its write lock past the writer's busy
/// timeout, as a full disk or an operator's own session would) leaves its channel blocked, and is
/// tried again until the file takes it: then the channel's rows are gone, and it is served again.
#[test]
fn an_erase_the_file_refuses_is_tried_again_until_it_is_carried_out() {
    let fx = Fixture::new("cache-erase-retry");
    fx.list((1..=6).map(m).collect(), T0 + 7 * STEP, None);
    let (release, locker) = super::tests::hold_write_lock(&fx.cache);
    let applied = fx.cache.stats().ops_applied;
    fx.cache.erase_channels(std::slice::from_ref(&fx.chan));
    let started = std::time::Instant::now();
    while fx.cache.stats().ops_applied == applied {
        assert!(started.elapsed() < WAIT, "the erase was never tried");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fx.cache.newest(&fx.chan, &Scope::Main, 100),
        Read::Blocked,
        "an erase the file refused keeps its channel blocked"
    );
    release.send(()).expect("release");
    locker.join().expect("locker");
    let started = std::time::Instant::now();
    loop {
        match fx.cache.newest(&fx.chan, &Scope::Main, 100) {
            Read::Served(rows) => {
                assert!(rows.is_empty(), "served again, and nothing of it held");
                break;
            }
            Read::Blocked => {}
            other => panic!("{other:?}"),
        }
        assert!(
            started.elapsed() < WAIT,
            "the refused erase was never tried again"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    fx.settle();
    assert_eq!(fx.cache.counts().served().expect("counts").messages, 0);
}

/// The text the cache holds for `id` in `fx`'s channel, read for a lookup by id as `shape` reads
/// it.
fn held_by_id(fx: &Fixture, id: &str, shape: observe::Shape) -> Option<String> {
    let ask = shadow::Ask::Message {
        consumer: shadow::Consumer::MessageById,
        id: MessageId(id.to_owned()),
    };
    let query = shadow::query(&ask, shape, false).expect("a query");
    let view = fx
        .cache
        .shadow_view(&fx.chan, &query)
        .served()
        .expect("served");
    view.rows.first().map(|r| r.message.content.clone())
}

/// An adapter may push full events for a provider this server does not poll, a Slack or a
/// Discord channel included, and its event may carry no edit time: the contract lets a copy that
/// came by a path that cannot say leave `edited_timestamp` out. A message edited once (a read
/// brought its edit time), then edited again, the adapter pushing the second edit with no edit
/// time: the cache must take the text it was told, and never vouch for the text edited away. The
/// poller's copy, which its own read brought, reports edits as the provider's reads do, so one
/// with no edit time is still the older version. (The review's probe
/// `rr1_an_adapter_pushed_edit_without_an_edit_time_is_not_thrown_away`.)
#[test]
fn an_adapters_pushed_edit_without_an_edit_time_is_never_thrown_away() {
    use crate::live::{LiveKind, LiveMessage};
    for shape in [observe::Shape::Slack, observe::Shape::DiscordNative] {
        for by in [observe::PushedBy::Adapter, observe::PushedBy::Poller] {
            let fx = Fixture::new("cache-adapter-edit");
            let first = T0 + 6 * STEP + SEC;
            fx.clock.set(first);
            let mut rows: Vec<Message> = (1..=6).map(m).collect();
            rows[2].content = "first edit".to_owned();
            rows[2].edited_timestamp = Some(crate::clock::iso_from_ms(T0 + 5 * STEP));
            let id = rows[2].id.0.clone();
            fx.list(
                rows.clone(),
                first,
                Some(newest_proof(ListStart::At(T0 + STEP))),
            );
            // The second edit is pushed a minute later, with no edit time.
            let at = first + 60 * SEC;
            fx.clock.set(at);
            let mut edited = rows[2].clone();
            edited.content = "second edit".to_owned();
            edited.edited_timestamp = None;
            let live = LiveMessage {
                event_id: "e1".to_owned(),
                kind: LiveKind::Update,
                historical: false,
                message: edited,
                self_posted: false,
            };
            fx.offer(&fx.chan, observe::pushed(shape, &live, by, at));
            let content = held_by_id(&fx, &id, shape);
            match by {
                observe::PushedBy::Adapter => {
                    assert_eq!(
                        content.as_deref(),
                        Some("second edit"),
                        "{shape:?}: the cache threw away an edit the adapter pushed"
                    );
                }
                observe::PushedBy::Poller => {
                    assert_eq!(
                        content.as_deref(),
                        Some("first edit"),
                        "{shape:?}: a copy the poller read with no edit time is the older version"
                    );
                }
            }
        }
    }
}

/// Wait, for at most [`WAIT`], until the writer has applied `count` more operations than
/// `applied`.
fn wait_applied(cache: &MessageCache, applied: u64, count: u64, what: &str) {
    let started = std::time::Instant::now();
    while cache.stats().ops_applied < applied + count {
        assert!(started.elapsed() < WAIT, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Wait, for at most [`WAIT`], until `chan`'s Flat list is served again, and hand it back.
fn served_again(cache: &MessageCache, chan: &ChanKey, what: &str) -> Vec<String> {
    let started = std::time::Instant::now();
    loop {
        match cache.newest(chan, &Scope::Flat, 100) {
            Read::Served(rows) => return rows.into_iter().map(|r| r.message.id.0).collect(),
            Read::Blocked => {}
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < WAIT, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A pushed deletion of m3, seen at `observed_ms`, offered under `stamp`.
fn offer_deletion_of_m3(cache: &MessageCache, chan: &ChanKey, stamp: Stamp, observed_ms: i64) {
    cache.offer(
        chan.clone(),
        stamp,
        Observation::Push {
            push: Push::Delete {
                id: MessageId("m3".to_owned()),
                created_ms: None,
            },
            observed_ms,
        },
    );
}

/// A sweep the file refuses (another program holding its lock past the writer's busy timeout, as
/// a full disk would) has already moved the epochs of the channels it leaves out, so what was on
/// its way to the file for them from before it, a pushed deletion included, is dropped unwritten.
/// A live-fed bridge's channel leaves the list and comes back before the file takes writes again:
/// the refused sweep must still erase it, and keep it blocked until then, whatever the snapshots
/// after it say. A third snapshot naming it, as the second did, would otherwise count it as routed
/// throughout. Kept and served, the channel would hold the deleted m3 for the feed to vouch for.
/// (The review's probe
/// `review_probe_a_channel_readded_after_a_refused_sweep_never_vouches_for_a_dropped_deletion`,
/// and a third snapshot.)
#[test]
fn a_channel_put_back_after_a_refused_sweep_is_still_erased_and_never_vouches_for_a_lost_deletion()
{
    let dir = crate::testing::TempDir::new("cache-refused-sweep");
    let first = T0 + 8 * STEP + 1_000;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    clock.set(first + 10_000);
    // A pushed deletion of m3 is received and stamped.
    let stamp = cache.stamp(&chan);
    let without = RouteSnapshot::new(Vec::<ChanKey>::new(), [chan.provider_key.clone()]);
    let with = RouteSnapshot::new([chan.clone()], [chan.provider_key.clone()]);
    // The file refuses writes for a while, and the writer is behind: the channel leaves the list,
    // and comes back, before it gets to either sweep.
    let (release, locker) = super::tests::hold_write_lock(&cache);
    let applied = cache.stats().ops_applied;
    let resume = cache.pause_writer();
    cache.retain_only(without);
    cache.retain_only(with.clone());
    drop(resume);
    wait_applied(&cache, applied, 3, "the sweeps were never tried");
    // A third snapshot names the channel again, as the one before it did.
    cache.retain_only(with);
    wait_applied(&cache, applied, 4, "the third sweep was never tried");
    assert_eq!(
        cache.newest(&chan, &Scope::Flat, 100),
        Read::Blocked,
        "a channel a refused sweep left out is served before that sweep's erase of it"
    );
    release.send(()).expect("release");
    locker.join().expect("locker");
    // The deletion lands now, under its stamp from before the sweep.
    offer_deletion_of_m3(&cache, &chan, stamp, first + 10_000);
    let held = served_again(&cache, &chan, "the refused sweep was never carried out");
    assert!(held.is_empty(), "the channel was erased: {held:?}");
    clock.set(first + 20_000);
    let (decision, content) = decide_m3(&cache, &chan, first + 20_000);
    assert_eq!(content, None, "a deleted message kept: {decision:?}");
    assert_ne!(decision, shadow::Decision::Serve);
}

/// An erase of a channel the file refused is carried out later, and detaches with the channel's
/// rows the tombstones of deletions written since it was asked for. A read begun before then, but
/// after the erase was asked for, must not land after it: in a channel that no longer remembers
/// the deletion, it would bring the deleted message back for the feed to vouch for.
#[test]
fn a_read_begun_before_a_refused_erase_was_carried_out_never_lands_after_it() {
    let dir = crate::testing::TempDir::new("cache-refused-erase-read");
    let first = T0 + 8 * STEP + 1_000;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    let (release, locker) = super::tests::hold_write_lock(&cache);
    let applied = cache.stats().ops_applied;
    clock.set(first + 5_000);
    cache.erase_channels(std::slice::from_ref(&chan));
    // A read of the channel's Flat list begins after the erase was asked for.
    let read_began = first + 10_000;
    clock.set(read_began);
    let read_stamp = cache.stamp(&chan);
    wait_applied(&cache, applied, 1, "the erase was never tried");
    // Held still once the erase and its first retry are refused, so that what follows lands
    // before the next retry.
    let resume = cache.pause_writer();
    release.send(()).expect("release");
    locker.join().expect("locker");
    // m3 is deleted upstream after the read began, and the deletion is written.
    clock.set(first + 20_000);
    let stamp = cache.stamp(&chan);
    offer_deletion_of_m3(&cache, &chan, stamp, first + 20_000);
    drop(resume);
    let held = served_again(&cache, &chan, "the refused erase was never carried out");
    assert!(held.is_empty(), "the channel was erased: {held:?}");
    // The read lands now, with m3 as it was when it began.
    let flat = Observation::Listing(Listing {
        scope: Scope::Flat,
        rows: (1..=8).map(|n| Incoming::new(m(n), Scope::Flat)).collect(),
        observed_ms: read_began,
        proof: Some(newest_proof(ListStart::At(T0 + STEP))),
    });
    cache.offer(chan.clone(), read_stamp, flat);
    assert!(cache.quiesce_blocking(WAIT));
    clock.set(first + 30_000);
    let (decision, content) = decide_m3(&cache, &chan, first + 30_000);
    assert!(
        !(decision == shadow::Decision::Serve && content.is_some()),
        "a message deleted upstream would be served by id: {content:?}"
    );
}
