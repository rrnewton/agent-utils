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
//!
//! And what the second review of the fix found:
//!
//! - **A copy dated before a whole-channel erase**: an erase of a whole channel (a confirmed loss
//!   of access, a provider erase, a sweep, at once or carried out late) forgets the deletions and
//!   edits the live feed delivered for it. A read begun after it, its stamp current, may still
//!   bring a copy its provider dated before them; the feed must not vouch for what that copy
//!   brings back.
//!
//! And what the third review of the fix found:
//!
//! - **A change the file never took**: the live feed's changes for a channel off the route list
//!   reach nothing, so a channel put back on the list, or added to it, is in doubt from when it is
//!   listed; a change offered for a channel off the list, or before the first list came, is
//!   recorded as left out; and the feed vouches for nothing seen before the first list reached the
//!   cache, nor before an erased file was replaced.
//! - **A change a whole erase discards**: one on its way to the file when an erase of its whole
//!   channel moved the channel's epoch is recorded as left out, whether or not the file held a row
//!   of the channel for the erase to record a drop on.
//!
//! And what the fourth review of the fix found:
//!
//! - **A pushed edit the file keeps out**: rule 5 keeps the text a read brought over a pushed edit
//!   that is a strict prefix of it, taking it for a bridge's snippet, but it may as well be the
//!   whole of an edit that cut the end off; and rule 2 keeps it over a pushed edit seen in the same
//!   millisecond as that read. Either way the file may lack a change the feed delivered, so it is
//!   recorded as left out, and the feed never vouches for the text the edit took away.
//! - **A pushed text taken for whole**: whatever text a push brings may be a snippet, and is never
//!   served as the message until a read proves it whole; a pushed edit never makes a pushed text
//!   whole, and a read that began no later than the push never replaces it.
//! - **A pushed edit dropped as too old**: an edit of a message past the channel's age cutoff,
//!   which the file still holds until the hourly expiry takes it out, is recorded as left out.
//! - **A push lifting a drop**: no push vouches for a message a dropped change left in doubt, since
//!   it says nothing of whether that change came before it.
//!
//! And what the fifth review of the fix found:
//!
//! - **A read begun before an un-timed pushed edit**: a push with no edit time leaves the held
//!   edit time at the version before it, so a read begun no later than the push must not take a
//!   later edit time of its own for proof of a later version. Nor may a read begun earlier once a
//!   bridge's read that cannot say when its text was edited has replaced the held text, or proved
//!   a pushed one whole.

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
        hue: None,
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

/// How a whole channel is erased: each detaches its rows, its tombstones and its coverage with it.
#[derive(Clone, Copy, Debug)]
enum WholeErase {
    /// Access to the channel confirmed lost ([`MessageCache::erase_channels`]).
    Channel,
    /// The provider's credential refused for good ([`MessageCache::erase_provider`]).
    Provider,
    /// The owner takes the channel off the list in Settings and puts it back: a sweep.
    Sweep,
}

const WHOLE_ERASES: [WholeErase; 3] =
    [WholeErase::Channel, WholeErase::Provider, WholeErase::Sweep];

/// Erase `chan` whole, as `how` does, and wait until the writer has carried it out.
fn erase_whole(cache: &MessageCache, chan: &ChanKey, how: WholeErase) {
    match how {
        WholeErase::Channel => cache.erase_channels(std::slice::from_ref(chan)),
        WholeErase::Provider => cache.erase_provider(&chan.provider_key),
        WholeErase::Sweep => {
            cache.retain_only(RouteSnapshot::new(
                Vec::<ChanKey>::new(),
                [chan.provider_key.clone()],
            ));
            assert!(cache.quiesce_blocking(WAIT));
            cache.retain_only(RouteSnapshot::new(
                [chan.clone()],
                [chan.provider_key.clone()],
            ));
        }
    }
    assert!(cache.quiesce_blocking(WAIT));
}

/// A read of `chan`'s Flat list, m1 to m8 as first read, begun now under a stamp taken now, whose
/// provider dated its copy `as_of`: a bridge's copy it could not bring up to date, which the fill
/// dates then though the read began later (`Fill::observed`). `proven`: whether it proves the
/// newest stretch, or vouches for nothing but the rows.
fn read_dated(cache: &MessageCache, chan: &ChanKey, as_of: i64, proven: bool) {
    let rows = (1..=8).map(|n| Incoming::new(m(n), Scope::Flat)).collect();
    let observation = if proven {
        Observation::Listing(Listing {
            scope: Scope::Flat,
            rows,
            observed_ms: as_of,
            proof: Some(newest_proof(ListStart::At(T0 + STEP))),
        })
    } else {
        Observation::Rows {
            rows,
            scope: Some(Scope::Flat),
            observed_ms: as_of,
        }
    };
    let stamp = cache.stamp(chan);
    cache.offer(chan.clone(), stamp, observation);
    assert!(cache.quiesce_blocking(WAIT));
}

/// The feed of a live-fed bridge delivers a change of m3 at `first` + 10 s, and the file records
/// it: its deletion (a tombstone), or with `edit` an edit to `EDITED`. At `first` + 20 s the whole
/// channel is erased as `how` erases it (none for the control), the change with its rows. At
/// `first` + 30 s a read begins, its stamp current, and lands with its provider's copy dated
/// `first` + 5 s, before the change. The cache and the channel, the clock at `first` + 30 s.
fn change_erase_then_older_copy(
    dir: &crate::testing::TempDir,
    clock: &Arc<ManualClock>,
    first: i64,
    how: Option<WholeErase>,
    edit: bool,
    proven: bool,
) -> (MessageCache, ChanKey) {
    let (cache, chan) = bridge_with_flat(dir, clock, first);
    let changed_at = first + 10 * SEC;
    clock.set(changed_at);
    let stamp = cache.stamp(&chan);
    if edit {
        let mut changed = m(3);
        changed.content = "EDITED".to_owned();
        cache.offer(
            chan.clone(),
            stamp,
            Observation::Push {
                push: Push::Upsert {
                    message: Box::new(Incoming::new(changed, Scope::Flat)),
                    update: true,
                    partial: false,
                },
                observed_ms: changed_at,
            },
        );
    } else {
        offer_deletion_of_m3(&cache, &chan, stamp, changed_at);
    }
    assert!(cache.quiesce_blocking(WAIT));
    let held = decide_m3(&cache, &chan, changed_at + 1).1;
    let expected = edit.then(|| "EDITED".to_owned());
    assert_eq!(held, expected, "the feed's change landed");
    if let Some(how) = how {
        clock.set(first + 20 * SEC);
        erase_whole(&cache, &chan, how);
    }
    clock.set(first + 30 * SEC);
    read_dated(&cache, &chan, first + 5 * SEC, proven);
    (cache, chan)
}

/// Whether the cache would vouch, by id, for m3 as the feed's change left it: deleted (no text),
/// or with `edit` holding `EDITED`. A decision that serves anything else vouches for a change the
/// feed delivered being undone.
fn vouches_for_undone_change(
    decision: shadow::Decision,
    content: Option<&str>,
    edit: bool,
) -> bool {
    decision == shadow::Decision::Serve
        && if edit {
            content != Some("EDITED")
        } else {
            content.is_some()
        }
}

/// The control: with no erase, the deletion's tombstone holds the older copy of m3 back, and the
/// edit's later observation keeps its text.
#[test]
fn control_an_older_copy_with_no_erase_never_undoes_the_feeds_change() {
    for edit in [false, true] {
        let dir = crate::testing::TempDir::new("cache-older-copy-control");
        let first = T0 + 8 * STEP + SEC;
        let clock = Arc::new(ManualClock(AtomicI64::new(first)));
        let (cache, chan) = change_erase_then_older_copy(&dir, &clock, first, None, edit, true);
        let asked = first + 40 * SEC;
        clock.set(asked);
        let (decision, content) = decide_m3(&cache, &chan, asked);
        assert!(
            !vouches_for_undone_change(decision, content.as_deref(), edit),
            "edit {edit}: {decision:?} {content:?}"
        );
    }
}

/// A whole-channel erase detaches the tombstone of a deletion the live feed delivered. A read begun
/// after the erase, its stamp current, but answered from a bridge's copy dated before the deletion,
/// lands in a channel that no longer remembers it: the deleted m3 comes back, with nothing to hold
/// it. The feed must not vouch for it: not ten seconds on, while the read's own claim would, nor a
/// day and an hour on, past [`shadow::REVALIDATED_BRIDGE_MS`], where only the feed could, for as
/// long as the process runs. By every erase that detaches a channel: a confirmed loss of access, a
/// provider erase, and a sweep (the channel taken off the list and put back). (The reviews'
/// probes `rv2_a_channel_erase_then_a_bridge_copy_dated_before_a_deletion_never_vouches_for_it`,
/// `rv2_d_a_channel_swept_and_put_back_then_a_bridge_copy_dated_before_a_deletion_never_vouches_for_it`,
/// `rv2_a_snapshot_older_than_a_channel_erase_never_brings_back_a_deleted_message` and
/// `rv2_a_snapshot_older_than_a_sweep_never_brings_back_a_deleted_message`.)
#[test]
fn a_copy_dated_before_a_whole_channel_erase_never_brings_back_a_deletion_the_feed_delivered() {
    let mut vouched = Vec::new();
    for how in WHOLE_ERASES {
        let dir = crate::testing::TempDir::new("cache-older-copy-deleted");
        let first = T0 + 8 * STEP + SEC;
        let clock = Arc::new(ManualClock(AtomicI64::new(first)));
        let (cache, chan) =
            change_erase_then_older_copy(&dir, &clock, first, Some(how), false, true);
        for asked in [first + 40 * SEC, first + 25 * HOUR] {
            clock.set(asked);
            let (decision, content) = decide_m3(&cache, &chan, asked);
            if vouches_for_undone_change(decision, content.as_deref(), false) {
                vouched.push(format!(
                    "{how:?}, asked {} s on: {decision:?} {content:?}",
                    (asked - first) / SEC
                ));
            }
        }
    }
    assert!(
        vouched.is_empty(),
        "a message deleted upstream, its deletion delivered by the feed, is vouched for: {vouched:#?}"
    );
}

/// The same for an edit the feed delivered: the copy from before it brings the old text back, and
/// the old text must never be vouched for. (The review's probe
/// `rv2_b_channel_erase_then_a_bridge_copy_dated_before_an_edit_never_vouches_for_the_old_text`.)
#[test]
fn a_copy_dated_before_a_whole_channel_erase_never_brings_back_text_the_feed_edited_away() {
    let mut vouched = Vec::new();
    for how in WHOLE_ERASES {
        let dir = crate::testing::TempDir::new("cache-older-copy-edited");
        let first = T0 + 8 * STEP + SEC;
        let clock = Arc::new(ManualClock(AtomicI64::new(first)));
        let (cache, chan) =
            change_erase_then_older_copy(&dir, &clock, first, Some(how), true, true);
        for asked in [first + 40 * SEC, first + 25 * HOUR] {
            clock.set(asked);
            let (decision, content) = decide_m3(&cache, &chan, asked);
            if vouches_for_undone_change(decision, content.as_deref(), true) {
                vouched.push(format!(
                    "{how:?}, asked {} s on: {decision:?} {content:?}",
                    (asked - first) / SEC
                ));
            }
        }
    }
    assert!(
        vouched.is_empty(),
        "text edited away is vouched for: {vouched:#?}"
    );
}

/// The feed's own promise, with no claim beside it: the older copy proves nothing, so only the
/// feed's vouching window could vouch for it. That window runs from the file's opening, and a
/// whole-channel erase forgets what the feed delivered since: it must not vouch for a copy dated
/// before the erase. (The review's probe
/// `rv2_fed_since_vouches_for_a_copy_dated_before_a_channel_erase_forgot_its_deletion`.)
#[test]
fn the_feed_alone_never_vouches_for_a_copy_dated_before_a_whole_channel_erase() {
    let mut vouched = Vec::new();
    for how in WHOLE_ERASES {
        for edit in [false, true] {
            let dir = crate::testing::TempDir::new("cache-older-copy-fed");
            let first = T0 + 8 * STEP + SEC;
            let clock = Arc::new(ManualClock(AtomicI64::new(first)));
            let (cache, chan) =
                change_erase_then_older_copy(&dir, &clock, first, Some(how), edit, false);
            let asked = first + 40 * SEC;
            clock.set(asked);
            let (decision, content) = decide_m3(&cache, &chan, asked);
            if vouches_for_undone_change(decision, content.as_deref(), edit) {
                vouched.push(format!("{how:?}, edit {edit}: {decision:?} {content:?}"));
            }
        }
    }
    assert!(
        vouched.is_empty(),
        "the feed vouches for a copy dated before the erase: {vouched:#?}"
    );
}

/// The erase of a channel the file refused, carried out later, forgets the deletion the feed
/// delivered before it as surely: a copy dated before it, read after it, must not bring m3 back
/// for the feed to vouch for.
#[test]
fn a_copy_dated_before_an_erase_carried_out_late_never_brings_back_a_deletion_the_feed_delivered() {
    let dir = crate::testing::TempDir::new("cache-older-copy-owed");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    clock.set(first + 10 * SEC);
    let stamp = cache.stamp(&chan);
    offer_deletion_of_m3(&cache, &chan, stamp, first + 10 * SEC);
    assert!(cache.quiesce_blocking(WAIT));
    let (release, locker) = super::tests::hold_write_lock(&cache);
    let applied = cache.stats().ops_applied;
    clock.set(first + 20 * SEC);
    cache.erase_channels(std::slice::from_ref(&chan));
    wait_applied(&cache, applied, 1, "the erase was never tried");
    release.send(()).expect("release");
    locker.join().expect("locker");
    let held = served_again(&cache, &chan, "the refused erase was never carried out");
    assert!(held.is_empty(), "the channel was erased: {held:?}");
    clock.set(first + 30 * SEC);
    read_dated(&cache, &chan, first + 5 * SEC, true);
    for asked in [first + 40 * SEC, first + 25 * HOUR] {
        clock.set(asked);
        let (decision, content) = decide_m3(&cache, &chan, asked);
        assert!(
            !vouches_for_undone_change(decision, content.as_deref(), false),
            "asked {} s on: {decision:?} {content:?}",
            (asked - first) / SEC
        );
    }
}

/// What a whole-channel erase leaves in doubt is lifted as on any drop: a read that began after it
/// vouches for what it lists, and its listing takes out the older copy's m3, which it did not list.
#[test]
fn a_read_begun_after_a_whole_channel_erase_is_vouched_for_again() {
    let dir = crate::testing::TempDir::new("cache-older-copy-lifted");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) =
        change_erase_then_older_copy(&dir, &clock, first, Some(WholeErase::Channel), false, true);
    clock.set(first + 40 * SEC);
    let flat = Observation::Listing(Listing {
        scope: Scope::Flat,
        rows: (1..=8)
            .filter(|&n| n != 3)
            .map(|n| Incoming::new(m(n), Scope::Flat))
            .collect(),
        observed_ms: first + 40 * SEC,
        proof: Some(newest_proof(ListStart::At(T0 + STEP))),
    });
    let stamp = cache.stamp(&chan);
    cache.offer(chan.clone(), stamp, flat);
    assert!(cache.quiesce_blocking(WAIT));
    let asked = first + 50 * SEC;
    clock.set(asked);
    let (decision, content) = decide_m3(&cache, &chan, asked);
    assert_eq!(content, None, "the read took m3 out: {decision:?}");
    let ask = shadow::Ask::Message {
        consumer: shadow::Consumer::MessageById,
        id: m(1).id,
    };
    let query = shadow::query(&ask, observe::Shape::Bridge, true).expect("a query");
    let view = cache.shadow_view(&chan, &query).served().expect("served");
    let facts = shadow::Facts {
        asked_ms: asked,
        live_fed: true,
        fed_since_ms: Some(cache.fed_since_ms()),
        dropped_ms: cache.last_drop_ms(&chan),
        barrier: true,
        feed: shadow::Feed::Deletions,
    };
    assert_eq!(
        shadow::decide(&ask, &view, &facts),
        shadow::Decision::Serve,
        "m1, listed by a read begun after the erase, is vouched for again"
    );
}

/// A change of m3 the live feed delivers at `at`, under `stamp`: its deletion, or with `edit` an
/// edit to `EDITED`.
fn offer_change_of_m3(cache: &MessageCache, chan: &ChanKey, stamp: Stamp, at: i64, edit: bool) {
    if edit {
        let mut changed = m(3);
        changed.content = "EDITED".to_owned();
        cache.offer(
            chan.clone(),
            stamp,
            Observation::Push {
                push: Push::Upsert {
                    message: Box::new(Incoming::new(changed, Scope::Flat)),
                    update: true,
                    partial: false,
                },
                observed_ms: at,
            },
        );
    } else {
        offer_deletion_of_m3(cache, chan, stamp, at);
    }
}

/// Every way the cache would vouch for m3 as the feed's change left it undone, asked 50 seconds
/// after `first`, while a proven copy's claim stands, and a day and an hour after, past
/// [`shadow::REVALIDATED_BRIDGE_MS`], where only the feed could vouch. `what` names the case.
fn undone_when_asked(
    cache: &MessageCache,
    chan: &ChanKey,
    clock: &ManualClock,
    first: i64,
    edit: bool,
    what: &str,
) -> Vec<String> {
    let mut vouched = Vec::new();
    for asked in [first + 50 * SEC, first + 25 * HOUR] {
        clock.set(asked);
        let (decision, content) = decide_m3(cache, chan, asked);
        if vouches_for_undone_change(decision, content.as_deref(), edit) {
            vouched.push(format!(
                "{what}, asked {} s on: {decision:?} {content:?}",
                (asked - first) / SEC
            ));
        }
    }
    vouched
}

/// The routes naming exactly `chans`, of provider `p`.
fn routes_of(chans: &[ChanKey]) -> RouteSnapshot {
    RouteSnapshot::new(chans.to_vec(), ["p".to_owned()])
}

/// The owner takes C1 of a live-fed bridge off the list at `first` + 10 s (a sweep, which records a
/// drop as it erases the channel) and puts it back at + 30 s. At + 20 s, while it is off the list,
/// m3 is changed upstream, deleted or edited: the sink offers nothing for a channel off its list,
/// and the ingest route refuses an adapter's push for it (`offered` false); or the change reaches
/// the cache all the same (`offered` true), where nothing of the channel is written. A read begun
/// at + 40 s lands the bridge's copy dated + 15 s: after the sweep's drop, before the change. The
/// feed must not vouch for m3 as that copy has it, nor may the copy's claim. (The reviews' probes
/// `r3_p1_off_list_change` and
/// `rv3_f1_a_deletion_made_while_the_channel_was_off_the_list_is_never_vouched_for`.)
#[test]
fn a_change_made_while_the_channel_was_off_the_list_is_never_undone_by_a_copy_from_then() {
    let mut vouched = Vec::new();
    for edit in [false, true] {
        for proven in [true, false] {
            for offered in [false, true] {
                let dir = crate::testing::TempDir::new("cache-off-the-list");
                let first = T0 + 8 * STEP + SEC;
                let clock = Arc::new(ManualClock(AtomicI64::new(first)));
                let (cache, chan) = bridge_with_flat(&dir, &clock, first);
                clock.set(first + 10 * SEC);
                cache.retain_only(routes_of(&[]));
                assert!(cache.quiesce_blocking(WAIT));
                clock.set(first + 20 * SEC);
                if offered {
                    let stamp = cache.stamp(&chan);
                    offer_change_of_m3(&cache, &chan, stamp, first + 20 * SEC, edit);
                    assert!(cache.quiesce_blocking(WAIT));
                }
                clock.set(first + 30 * SEC);
                cache.retain_only(routes_of(std::slice::from_ref(&chan)));
                assert!(cache.quiesce_blocking(WAIT));
                clock.set(first + 40 * SEC);
                read_dated(&cache, &chan, first + 15 * SEC, proven);
                vouched.extend(undone_when_asked(
                    &cache,
                    &chan,
                    &clock,
                    first,
                    edit,
                    &format!("edit {edit}, proven {proven}, offered {offered}"),
                ));
            }
        }
    }
    assert!(
        vouched.is_empty(),
        "a change made while the channel was off the list is undone and vouched for: {vouched:#?}"
    );
}

/// The same with no sweep at all: C1 is first put on the list at `first` + 20 s, the file having
/// been opened, and C0 listed, at `first`. m3 was changed at + 10 s, before C1 was listed (offered
/// to the cache or not); the bridge's copy is dated + 5 s. (The reviews' probes
/// `r3_p2_unlisted_change` and
/// `rv3_f1_a_channel_listed_after_the_file_opened_never_vouches_for_a_copy_from_before`.)
#[test]
fn a_channel_listed_after_the_file_opened_never_vouches_for_a_copy_from_before() {
    let mut vouched = Vec::new();
    for edit in [false, true] {
        for proven in [true, false] {
            for offered in [false, true] {
                let dir = crate::testing::TempDir::new("cache-listed-late");
                let first = T0 + 8 * STEP + SEC;
                let clock = Arc::new(ManualClock(AtomicI64::new(first)));
                let chan = ChanKey::new("p", "C1");
                let other = ChanKey::new("p", "C0");
                let cache = fixtures::open(&dir, &clock, Limits::default());
                cache.retain_only(routes_of(std::slice::from_ref(&other)));
                assert!(cache.quiesce_blocking(WAIT));
                clock.set(first + 10 * SEC);
                if offered {
                    let stamp = cache.stamp(&chan);
                    offer_change_of_m3(&cache, &chan, stamp, first + 10 * SEC, edit);
                    assert!(cache.quiesce_blocking(WAIT));
                }
                clock.set(first + 20 * SEC);
                cache.retain_only(routes_of(&[other, chan.clone()]));
                assert!(cache.quiesce_blocking(WAIT));
                clock.set(first + 30 * SEC);
                read_dated(&cache, &chan, first + 5 * SEC, proven);
                vouched.extend(undone_when_asked(
                    &cache,
                    &chan,
                    &clock,
                    first,
                    edit,
                    &format!("edit {edit}, proven {proven}, offered {offered}"),
                ));
            }
        }
    }
    assert!(
        vouched.is_empty(),
        "a change made before the channel was listed is undone and vouched for: {vouched:#?}"
    );
}

/// Before the first route snapshot (a server whose list is not yet known, or not yet armed), a
/// change of m3 reaches the cache: a deletion of a message the file does not hold, or an edit.
/// Nothing is written. The first snapshot, at `first` + 20 s, lists C1; a read begun at + 30 s
/// lands the bridge's copy dated + 5 s, before the change. (The review's probe
/// `r3_p2b_pre_first_sweep_deletion`.)
#[test]
fn a_change_offered_before_the_first_snapshot_is_never_undone_by_a_copy_from_before_it() {
    let mut vouched = Vec::new();
    for edit in [false, true] {
        for proven in [true, false] {
            let dir = crate::testing::TempDir::new("cache-before-first-sweep");
            let first = T0 + 8 * STEP + SEC;
            let clock = Arc::new(ManualClock(AtomicI64::new(first)));
            let chan = ChanKey::new("p", "C1");
            let cache = fixtures::open(&dir, &clock, Limits::default());
            clock.set(first + 10 * SEC);
            let stamp = cache.stamp(&chan);
            offer_change_of_m3(&cache, &chan, stamp, first + 10 * SEC, edit);
            assert!(cache.quiesce_blocking(WAIT));
            clock.set(first + 20 * SEC);
            cache.retain_only(routes_of(std::slice::from_ref(&chan)));
            assert!(cache.quiesce_blocking(WAIT));
            clock.set(first + 30 * SEC);
            read_dated(&cache, &chan, first + 5 * SEC, proven);
            vouched.extend(undone_when_asked(
                &cache,
                &chan,
                &clock,
                first,
                edit,
                &format!("edit {edit}, proven {proven}"),
            ));
        }
    }
    assert!(
        vouched.is_empty(),
        "a change offered before the first snapshot is undone and vouched for: {vouched:#?}"
    );
}

/// Between the file's opening, at `first`, and the first route snapshot, at + 20 s, nothing the
/// feed delivers reaches the cache at all: the sink offers nothing for a channel it does not yet
/// know, and nothing before the cache is installed behind it. m3 deleted upstream at + 10 s leaves
/// no trace, so the feed vouches for nothing seen before that snapshot: not for m3 in a copy dated
/// + 5 s that claims nothing.
#[test]
fn the_feed_vouches_for_nothing_seen_before_the_first_route_snapshot() {
    let dir = crate::testing::TempDir::new("cache-fed-from-first-snapshot");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let chan = ChanKey::new("p", "C1");
    let cache = fixtures::open(&dir, &clock, Limits::default());
    clock.set(first + 20 * SEC);
    cache.retain_only(routes_of(std::slice::from_ref(&chan)));
    assert!(cache.quiesce_blocking(WAIT));
    clock.set(first + 30 * SEC);
    read_dated(&cache, &chan, first + 5 * SEC, false);
    let vouched = undone_when_asked(&cache, &chan, &clock, first, false, "rows only");
    assert!(
        vouched.is_empty(),
        "the feed vouches for a copy from before the route list reached the cache: {vouched:#?}"
    );
    assert_eq!(cache.fed_since_ms(), first + 20 * SEC);
}

/// A clock a test moves by hand that runs `hook`, once, at the first reading taken on `thread`
/// after it is set, handing back the time from before the hook ran.
struct HookClock {
    now: Arc<AtomicI64>,
    thread: std::thread::ThreadId,
    hook: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Clock for HookClock {
    fn now_ms(&self) -> i64 {
        let now = self.now.load(std::sync::atomic::Ordering::SeqCst);
        if std::thread::current().id() == self.thread {
            let hook = lock(&self.hook).take();
            if let Some(hook) = hook {
                hook();
            }
        }
        now
    }
}

/// While an erased file is replaced, every offer comes back `Off`, recorded nowhere: the feed
/// vouches for nothing seen before the fresh file was installed. m3 is deleted upstream at
/// `first` + 10 s, while the fresh file is being opened, and its push is refused; the bridge's copy
/// dated + 5 s, which claims nothing, must not then be vouched for.
#[test]
fn the_feed_vouches_for_nothing_seen_before_an_erased_file_was_replaced() {
    let dir = crate::testing::TempDir::new("cache-fed-from-reopen");
    let first = T0 + 8 * STEP + SEC;
    let now = Arc::new(AtomicI64::new(first));
    let clock = Arc::new(HookClock {
        now: Arc::clone(&now),
        thread: std::thread::current().id(),
        hook: std::sync::Mutex::new(None),
    });
    let cache = Arc::new(
        MessageCache::open(CacheOptions {
            clock: Arc::clone(&clock) as Arc<dyn Clock>,
            ..CacheOptions::new(dir.path().join("cache").join("c.sqlite3"))
        })
        .expect("the cache opens"),
    );
    let chan = ChanKey::new("p", "C1");
    cache.retain_only(routes_of(std::slice::from_ref(&chan)));
    assert!(cache.quiesce_blocking(WAIT));
    let offered = Arc::new(std::sync::Mutex::new(None));
    {
        let (cache, chan, now, offered) = (
            Arc::downgrade(&cache),
            chan.clone(),
            Arc::clone(&now),
            Arc::clone(&offered),
        );
        *lock(&clock.hook) = Some(Box::new(move || {
            let at = first + 10 * SEC;
            now.store(at, std::sync::atomic::Ordering::SeqCst);
            let cache = cache.upgrade().expect("the cache");
            let stamp = cache.stamp(&chan);
            *lock(&offered) = Some(cache.offer(
                chan.clone(),
                stamp,
                Observation::Push {
                    push: Push::Delete {
                        id: MessageId("m3".to_owned()),
                        created_ms: None,
                    },
                    observed_ms: at,
                },
            ));
        }));
    }
    cache
        .erase_file(true)
        .expect("the file is erased and replaced");
    assert!(
        lock(&offered).is_some(),
        "the deletion was pushed while the file was replaced"
    );
    assert!(cache.quiesce_blocking(WAIT));
    now.store(first + 30 * SEC, std::sync::atomic::Ordering::SeqCst);
    read_dated(&cache, &chan, first + 5 * SEC, false);
    let mut vouched = Vec::new();
    for asked in [first + 50 * SEC, first + 25 * HOUR] {
        now.store(asked, std::sync::atomic::Ordering::SeqCst);
        let (decision, content) = decide_m3(&cache, &chan, asked);
        if vouches_for_undone_change(decision, content.as_deref(), false) {
            vouched.push(format!(
                "offer {:?}, asked {} s on: {decision:?} {content:?}",
                lock(&offered),
                (asked - first) / SEC
            ));
        }
    }
    assert!(
        vouched.is_empty(),
        "the feed vouches for a copy from before the fresh file could take offers: {vouched:#?}"
    );
}

/// What a channel put back on the list leaves in doubt is lifted as on any drop: a read begun after
/// it was put back vouches for what it lists.
#[test]
fn a_read_begun_after_a_channel_was_put_back_is_vouched_for_again() {
    let dir = crate::testing::TempDir::new("cache-put-back-lifted");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    clock.set(first + 10 * SEC);
    cache.retain_only(routes_of(&[]));
    assert!(cache.quiesce_blocking(WAIT));
    clock.set(first + 30 * SEC);
    cache.retain_only(routes_of(std::slice::from_ref(&chan)));
    assert!(cache.quiesce_blocking(WAIT));
    assert_eq!(cache.last_drop_ms(&chan), Some(first + 30 * SEC));
    clock.set(first + 40 * SEC);
    read_dated(&cache, &chan, first + 40 * SEC, true);
    let asked = first + 50 * SEC;
    clock.set(asked);
    let (decision, content) = decide_m3(&cache, &chan, asked);
    assert_eq!(
        (decision, content.as_deref()),
        (shadow::Decision::Serve, Some("body number 3")),
        "m3, listed by a read begun after the channel was put back, is vouched for again"
    );
}

/// A change of m3 the feed delivers at `first` + 20 s is queued while the writer is behind (held
/// still here), and C1 is erased whole, as `how` erases it, before the writer reaches it (none for
/// the control): the erase moves the channel's epoch, so the change is dropped unwritten. The file
/// holds no row of C1 when the erase is carried out (`prior` false: nothing of C1 written yet;
/// `prior` true: an earlier whole erase took its row, and nothing was written since), so the erase
/// detaches nothing. A read begun at + 40 s lands the bridge's copy dated + 15 s, before the
/// change. What the cache would then vouch for, as [`undone_when_asked`] reports it.
fn change_dropped_by_an_erase(prior: bool, how: Option<WholeErase>, edit: bool) -> Vec<String> {
    let dir = crate::testing::TempDir::new("cache-erase-drops-change");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let chan = ChanKey::new("p", "C1");
    let cache = fixtures::open(&dir, &clock, Limits::default());
    cache.retain_only(routes_of(std::slice::from_ref(&chan)));
    assert!(cache.quiesce_blocking(WAIT));
    if prior {
        read_dated(&cache, &chan, first, true);
        clock.set(first + 10 * SEC);
        cache.erase_channels(std::slice::from_ref(&chan));
        assert!(cache.quiesce_blocking(WAIT));
    }
    let at = first + 20 * SEC;
    clock.set(at);
    let resume = cache.pause_writer();
    let stamp = cache.stamp(&chan);
    offer_change_of_m3(&cache, &chan, stamp, at, edit);
    clock.set(at + 1);
    match how {
        Some(WholeErase::Channel) => cache.erase_channels(std::slice::from_ref(&chan)),
        Some(WholeErase::Provider) => cache.erase_provider(&chan.provider_key),
        Some(WholeErase::Sweep) => {
            cache.retain_only(routes_of(&[]));
            cache.retain_only(routes_of(std::slice::from_ref(&chan)));
        }
        None => {}
    }
    drop(resume);
    assert!(cache.quiesce_blocking(WAIT));
    if how.is_some() {
        assert_eq!(cache.stats().ops_stale, 1, "the erase dropped the change");
    }
    clock.set(first + 40 * SEC);
    read_dated(&cache, &chan, first + 15 * SEC, true);
    undone_when_asked(
        &cache,
        &chan,
        &clock,
        first,
        edit,
        &format!("prior {prior}, {how:?}, edit {edit}"),
    )
}

/// The control: with no erase, the queued change lands, and holds the older copy back: a
/// deletion's tombstone, and an edit's text, held as possibly a snippet, which a read that began
/// before the push never replaces (merge rule 5; this edit was #297's remaining item (h)).
#[test]
fn control_a_queued_change_with_no_erase_lands_and_holds_an_older_copy_back() {
    let mut vouched = Vec::new();
    for prior in [false, true] {
        for edit in [false, true] {
            vouched.extend(change_dropped_by_an_erase(prior, None, edit));
        }
    }
    assert!(vouched.is_empty(), "{vouched:#?}");
}

/// A whole-channel erase that finds no row of the channel records no drop of its own, yet the
/// change it dropped unwritten was a deletion or an edit the file now lacks just the same: the
/// observation that would have made the row, and its tombstone. It must be recorded as left out,
/// or the feed vouches for m3 as the older copy has it. By every whole erase: a confirmed loss of
/// access, a provider erase and a sweep. (The reviews' probes
/// `r3_p3_stale_change_for_unheld_channel` and
/// `rv3_f2_a_whole_erase_finding_no_row_never_lets_a_stale_deletion_be_undone`.)
#[test]
fn a_change_a_whole_erase_drops_for_a_channel_with_no_row_is_never_undone() {
    let mut vouched = Vec::new();
    for prior in [false, true] {
        for how in WHOLE_ERASES {
            for edit in [false, true] {
                vouched.extend(change_dropped_by_an_erase(prior, Some(how), edit));
            }
        }
    }
    assert!(
        vouched.is_empty(),
        "a change a whole erase dropped is undone and vouched for: {vouched:#?}"
    );
}

// ---- the fourth review of the fix: what a push says of a message's text ------------------------

/// A change of message `msg`, its text now `text`, pushed at `at`: an edit with `update`, a
/// creation without.
fn push_text(
    cache: &MessageCache,
    chan: &ChanKey,
    msg: Message,
    text: &str,
    update: bool,
    at: i64,
) {
    let changed = Message {
        content: text.to_owned(),
        ..msg
    };
    let stamp = cache.stamp(chan);
    cache.offer(
        chan.clone(),
        stamp,
        Observation::Push {
            push: Push::Upsert {
                message: Box::new(Incoming::new(changed, Scope::Flat)),
                update,
                partial: false,
            },
            observed_ms: at,
        },
    );
    assert!(cache.quiesce_blocking(WAIT));
}

/// What the cache would decide for `id` by id on a live-fed bridge, asked at `asked_ms`, the text
/// it holds, and whether it holds that text as possibly a snippet.
fn decide_id(
    cache: &MessageCache,
    chan: &ChanKey,
    id: &str,
    asked_ms: i64,
) -> (shadow::Decision, Option<String>, Option<bool>) {
    let ask = shadow::Ask::Message {
        consumer: shadow::Consumer::MessageById,
        id: MessageId(id.to_owned()),
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
    let row = view.rows.first();
    (
        shadow::decide(&ask, &view, &facts),
        row.map(|r| r.message.content.clone()),
        row.map(|r| r.content_partial),
    )
}

/// The adapter pushes an edit of m3 whose text the file does not take. Rule 5 takes a pushed text
/// that is a strict prefix of the text a read brought for a bridge's snippet, and keeps the text it
/// holds. But that prefix may as well be the whole of an edit that cut the end off ("body number 3"
/// to "body number"), or a snippet of an edit that changed the end ("body num…" for "body
/// numeral"): the file then lacks a change the feed delivered, and must record it as left out, or
/// the feed (and, within the day, the claim of the read that brought the old text) vouches for text
/// the edit took away. Asked 5 s on, and a day and an hour on. (The review's probe
/// `p1_a_pushed_edit_that_shortens_the_text_is_never_vouched_for_as_the_old_text`.)
#[test]
fn a_pushed_edit_the_file_does_not_take_is_never_vouched_for_as_the_text_it_replaced() {
    let mut failures = Vec::new();
    for (label, new_text) in [
        ("an edit that cut the end off", "body number"),
        ("a snippet of an edit that changed the end", "body num…"),
    ] {
        let dir = crate::testing::TempDir::new("cache-pushed-prefix");
        let first = T0 + 8 * STEP + SEC;
        let clock = Arc::new(ManualClock(AtomicI64::new(first)));
        let (cache, chan) = bridge_with_flat(&dir, &clock, first);
        let at = first + 10 * SEC;
        clock.set(at);
        push_text(&cache, &chan, m(3), new_text, true, at);
        let (_, kept, _) = decide_id(&cache, &chan, "m3", at + 1);
        assert_eq!(
            kept.as_deref(),
            Some("body number 3"),
            "{label}: rule 5 keeps the text a read brought"
        );
        for asked in [at + 5 * SEC, first + 25 * HOUR] {
            clock.set(asked);
            let (decision, content, _) = decide_id(&cache, &chan, "m3", asked);
            if decision == shadow::Decision::Serve && content.as_deref() != Some(new_text) {
                failures.push(format!(
                    "{label}, asked {} s on: {decision:?} {content:?}, the feed having delivered \
                     {new_text:?} (drop {:?})",
                    (asked - first) / SEC,
                    cache.last_drop_ms(&chan)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The same edit, cutting the end off m3, and two hours on a read of the newest end lists m6 to m8
/// alone, its listing starting above m3. m3 rests on the first read's claim, which the feed keeps
/// current; a window of 8 must not hand back the text the edit took away. (The review's probe
/// `p1w_a_window_never_hands_back_text_a_pushed_shortening_edit_took_away`.)
#[test]
fn a_window_never_hands_back_text_a_pushed_edit_the_file_did_not_take_took_away() {
    let dir = crate::testing::TempDir::new("cache-pushed-prefix-window");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    let at = first + 10 * SEC;
    clock.set(at);
    push_text(&cache, &chan, m(3), "body number", true, at);
    let later = first + 2 * HOUR;
    clock.set(later);
    let top = Observation::Listing(Listing {
        scope: Scope::Flat,
        rows: (6..=8).map(|n| Incoming::new(m(n), Scope::Flat)).collect(),
        observed_ms: later,
        proof: Some(newest_proof(ListStart::At(T0 + 6 * STEP))),
    });
    let stamp = cache.stamp(&chan);
    cache.offer(chan.clone(), stamp, top);
    assert!(cache.quiesce_blocking(WAIT));
    let asked = later + 5 * SEC;
    clock.set(asked);
    let ask = window(8);
    let query = shadow::query(&ask, observe::Shape::Bridge, true).expect("a query");
    let view = cache.shadow_view(&chan, &query).served().expect("served");
    let facts = shadow::Facts {
        asked_ms: asked,
        live_fed: true,
        fed_since_ms: Some(cache.fed_since_ms()),
        dropped_ms: cache.last_drop_ms(&chan),
        barrier: true,
        feed: shadow::Feed::Deletions,
    };
    let decision = shadow::decide(&ask, &view, &facts);
    let m3 = view
        .rows
        .iter()
        .find(|r| r.message.id.0 == "m3")
        .map(|r| r.message.content.clone());
    assert!(
        !(decision == shadow::Decision::Serve && m3.as_deref() == Some("body number 3")),
        "a window {decision:?} hands back m3 as {m3:?}"
    );
}

/// A read begins at `at` and lands with m3 as `FIRST EDIT`; an adapter's edit of m3 to `EDITED`,
/// received in the same millisecond, lands after it. Rule 2 takes a copy over the held one only
/// when it was seen later, so the file keeps the read's text: the edit must then be recorded as
/// left out, never lost while the feed vouches for the text it replaced. #297's remaining item (f),
/// the review's probe `rv2_c_a_pushed_edit_tied_with_the_read_that_brought_the_old_text_is_not_lost`.
#[test]
fn a_pushed_edit_tied_with_the_read_that_brought_the_old_text_is_never_lost() {
    let dir = crate::testing::TempDir::new("cache-pushed-edit-tie");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    let at = first + 10 * SEC;
    clock.set(at);
    let read_stamp = cache.stamp(&chan);
    let push_stamp = cache.stamp(&chan);
    let read = Observation::Listing(Listing {
        scope: Scope::Flat,
        rows: (1..=8)
            .map(|n| {
                let mut row = m(n);
                if n == 3 {
                    row.content = "FIRST EDIT".to_owned();
                }
                Incoming::new(row, Scope::Flat)
            })
            .collect(),
        observed_ms: at,
        proof: Some(newest_proof(ListStart::At(T0 + STEP))),
    });
    cache.offer(chan.clone(), read_stamp, read);
    let edited = Message {
        content: "EDITED".to_owned(),
        ..m(3)
    };
    cache.offer(
        chan.clone(),
        push_stamp,
        Observation::Push {
            push: Push::Upsert {
                message: Box::new(Incoming::new(edited, Scope::Flat)),
                update: true,
                partial: false,
            },
            observed_ms: at,
        },
    );
    assert!(cache.quiesce_blocking(WAIT));
    let mut vouched = Vec::new();
    for asked in [at + 5 * SEC, first + 25 * HOUR] {
        clock.set(asked);
        let (decision, content, _) = decide_id(&cache, &chan, "m3", asked);
        if vouches_for_undone_change(decision, content.as_deref(), true) {
            vouched.push(format!(
                "asked {} s on: {decision:?} {content:?}",
                (asked - first) / SEC
            ));
        }
    }
    assert!(vouched.is_empty(), "the old text vouched for: {vouched:#?}");
}

/// A text a push brought may be a snippet (a bridge falls back to one when its live event has no
/// body), and the cache holds it as possibly one. It must never count it as the whole message: a
/// pushed creation of a message it did not hold (`the full…`); one followed by a pushed edit, which
/// may be a snippet too (`the fuller…`); and a pushed edit of a message a read brought, whose text
/// is no prefix of the old (`Body…`, a snippet of an edit that changed the start). Asked 5 s on,
/// and a day and an hour on, where only the feed could vouch. (The review's probes
/// `p2_a_partial_pushed_creation_is_never_served_as_the_whole_message` and
/// `p2b_a_pushed_update_never_makes_a_pushed_snippet_whole`.)
#[test]
fn a_text_a_push_brought_is_never_served_as_the_whole_message() {
    let mut failures = Vec::new();
    for case in [
        "a pushed creation",
        "a pushed creation and a pushed edit",
        "a pushed edit",
    ] {
        let dir = crate::testing::TempDir::new("cache-pushed-snippet");
        let first = T0 + 8 * STEP + SEC;
        let clock = Arc::new(ManualClock(AtomicI64::new(first)));
        let (cache, chan) = bridge_with_flat(&dir, &clock, first);
        let at = first + 10 * SEC;
        clock.set(at);
        let nine = message("m9", "ada", "the whole of message nine", T0 + 9 * STEP);
        let (id, text) = match case {
            "a pushed creation" => {
                push_text(&cache, &chan, nine, "the full…", false, at);
                ("m9", "the full…")
            }
            "a pushed creation and a pushed edit" => {
                push_text(&cache, &chan, nine.clone(), "the full…", false, at);
                clock.set(at + SEC);
                push_text(&cache, &chan, nine, "the fuller…", true, at + SEC);
                ("m9", "the fuller…")
            }
            _ => {
                push_text(&cache, &chan, m(3), "Body…", true, at);
                ("m3", "Body…")
            }
        };
        for asked in [at + 5 * SEC, first + 25 * HOUR] {
            clock.set(asked);
            let (decision, content, partial) = decide_id(&cache, &chan, id, asked);
            assert_eq!(
                content.as_deref(),
                Some(text),
                "{case}: the pushed text is held"
            );
            if partial != Some(true) || decision == shadow::Decision::Serve {
                failures.push(format!(
                    "{case}, asked {} s on: {decision:?} {content:?} partial {partial:?}",
                    (asked - first) / SEC
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// What proves a pushed text whole is a read: one that brings the same text, whenever it began (a
/// lookup by id here, begun before the push), or one begun after the push, whatever text it
/// brings. The message is then vouched for again.
#[test]
fn a_read_proves_a_pushed_text_whole() {
    for case in ["the same text, read earlier", "a listing begun later"] {
        let dir = crate::testing::TempDir::new("cache-pushed-proven");
        let first = T0 + 8 * STEP + SEC;
        let clock = Arc::new(ManualClock(AtomicI64::new(first)));
        let (cache, chan) = bridge_with_flat(&dir, &clock, first);
        let at = first + 10 * SEC;
        clock.set(at);
        let whole = "the whole of message nine";
        let nine = message("m9", "ada", whole, T0 + 9 * STEP);
        let pushed = if case == "a listing begun later" {
            "the whole…"
        } else {
            whole
        };
        push_text(&cache, &chan, nine.clone(), pushed, false, at);
        assert_eq!(decide_id(&cache, &chan, "m9", at + 1).2, Some(true));
        let observation = if case == "a listing begun later" {
            clock.set(at + 20 * SEC);
            Observation::Listing(Listing {
                scope: Scope::Flat,
                rows: (1..=8)
                    .map(m)
                    .chain([nine])
                    .map(|row| Incoming::new(row, Scope::Flat))
                    .collect(),
                observed_ms: at + 20 * SEC,
                proof: Some(newest_proof(ListStart::At(T0 + STEP))),
            })
        } else {
            clock.set(at + 20 * SEC);
            Observation::Rows {
                rows: vec![Incoming::new(nine, Scope::Flat)],
                scope: None,
                observed_ms: at - 5 * SEC,
            }
        };
        let stamp = cache.stamp(&chan);
        cache.offer(chan.clone(), stamp, observation);
        assert!(cache.quiesce_blocking(WAIT));
        let asked = at + 30 * SEC;
        clock.set(asked);
        assert_eq!(
            decide_id(&cache, &chan, "m9", asked),
            (shadow::Decision::Serve, Some(whole.to_owned()), Some(false)),
            "{case}"
        );
    }
}

/// No erase at all: a pushed edit of m3, a message the file does not hold yet (held as possibly a
/// snippet), and then a read lands the bridge's copy dated before the edit. That copy must not
/// replace the edit's text: it may be of the version before the edit. #297's remaining item (h),
/// the review's probe `rv3_nb_a_pushed_edit_of_an_unheld_message_is_never_undone_by_an_older_copy`.
#[test]
fn a_pushed_edit_of_an_unheld_message_is_never_undone_by_an_older_copy() {
    let dir = crate::testing::TempDir::new("cache-unheld-edit");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let chan = ChanKey::new("p", "C1");
    let cache = fixtures::open(&dir, &clock, Limits::default());
    cache.retain_only(routes_of(std::slice::from_ref(&chan)));
    assert!(cache.quiesce_blocking(WAIT));
    let at = first + 20 * SEC;
    clock.set(at);
    push_text(&cache, &chan, m(3), "EDITED", true, at);
    clock.set(first + 40 * SEC);
    read_dated(&cache, &chan, first + 15 * SEC, true);
    let vouched = undone_when_asked(&cache, &chan, &clock, first, true, "an unheld edit");
    assert!(vouched.is_empty(), "{vouched:#?}");
}

/// A channel keeps a day of messages. The adapter pushes an edit of m3 a day and a minute after m3
/// was written, before the hourly expiry has taken the row out: the writer drops the edit as too
/// old to keep, but the file still holds the row with the text the edit replaced. The feed
/// delivered a change the file lacks, so it must be recorded as left out, or the feed vouches for
/// the old text until the expiry comes. (The review's probe
/// `p3_an_edit_dropped_as_too_old_never_leaves_the_old_text_vouched_for`.)
#[test]
fn a_pushed_edit_dropped_as_too_old_never_leaves_the_text_it_replaced_vouched_for() {
    let dir = crate::testing::TempDir::new("cache-pushed-too-old");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let chan = ChanKey::new("p", "C1");
    let limits = Limits {
        max_days: 1,
        ..Limits::default()
    };
    let cache = fixtures::open(&dir, &clock, limits);
    cache.retain_only(routes_of(std::slice::from_ref(&chan)));
    assert!(cache.quiesce_blocking(WAIT));
    read_dated(&cache, &chan, first, true);
    // m3 was written at T0 + 3 STEP: a day and a minute later it is past the channel's cutoff.
    let at = T0 + 3 * STEP + 24 * HOUR + 60 * SEC;
    clock.set(at);
    let too_old = cache.stats().rows_too_old;
    push_text(&cache, &chan, m(3), "EDITED", true, at);
    assert_eq!(
        cache.stats().rows_too_old,
        too_old + 1,
        "the edit is dropped as too old"
    );
    let asked = at + 5 * SEC;
    clock.set(asked);
    let (decision, content, _) = decide_id(&cache, &chan, "m3", asked);
    assert!(
        !vouches_for_undone_change(decision, content.as_deref(), true),
        "{decision:?} {content:?}: the feed vouches for the text an edit it delivered replaced"
    );
}

/// A read by id of `msg` as it is now, begun at `observed_ms` under a stamp taken now.
fn read_by_id(cache: &MessageCache, chan: &ChanKey, msg: Message, observed_ms: i64) {
    let stamp = cache.stamp(chan);
    cache.offer(
        chan.clone(),
        stamp,
        Observation::Rows {
            rows: vec![Incoming::new(msg, Scope::Flat)],
            scope: None,
            observed_ms,
        },
    );
    assert!(cache.quiesce_blocking(WAIT));
}

/// An offer for a live-fed bridge's channel is dropped at `first` + 10 s (the queue full, say):
/// it may have been a deletion, and every message of the channel seen before is in doubt from
/// then. A push after that says nothing of whether the dropped change came before it: an event
/// retried, or delivered out of order, may come after the deletion that took its message out. So
/// no push lifts the doubt: not m3's text pushed again, which must not move the row's time past the
/// drop; nor a pushed edit of m3, nor a pushed creation of m9, a message the cache did not hold,
/// though a read begun before the drop then brings the text the push brought and proves it whole.
/// Only a read begun after the drop vouches for the message again. (The review's probe
/// `p4_a_push_never_lifts_the_doubt_a_dropped_deletion_left`.)
#[test]
fn a_push_never_lifts_the_doubt_a_dropped_change_left() {
    let mut vouched = Vec::new();
    for case in [
        "the same text pushed again",
        "a pushed edit a read from before the drop proves whole",
        "a pushed creation a read from before the drop proves whole",
    ] {
        let dir = crate::testing::TempDir::new("cache-push-after-drop");
        let first = T0 + 8 * STEP + SEC;
        let clock = Arc::new(ManualClock(AtomicI64::new(first)));
        let (cache, chan) = bridge_with_flat(&dir, &clock, first);
        clock.set(first + 10 * SEC);
        cache.mark_suspect(&chan);
        assert!(cache.quiesce_blocking(WAIT));
        assert_ne!(
            decide_id(&cache, &chan, "m3", first + 11 * SEC).0,
            shadow::Decision::Serve,
            "{case}: the control, in doubt after the drop"
        );
        let at = first + 20 * SEC;
        clock.set(at);
        let now = match case {
            "the same text pushed again" => {
                push_text(&cache, &chan, m(3), "body number 3", true, at);
                m(3)
            }
            "a pushed edit a read from before the drop proves whole" => {
                push_text(&cache, &chan, m(3), "EDITED", true, at);
                Message {
                    content: "EDITED".to_owned(),
                    ..m(3)
                }
            }
            _ => {
                let nine = message("m9", "ada", "the whole of message nine", T0 + 9 * STEP);
                push_text(&cache, &chan, nine.clone(), &nine.content, false, at);
                nine
            }
        };
        if case != "the same text pushed again" {
            clock.set(at + SEC);
            read_by_id(&cache, &chan, now.clone(), first + 5 * SEC);
        }
        let id = now.id.0.clone();
        for asked in [at + 5 * SEC, first + 25 * HOUR] {
            clock.set(asked);
            let (decision, content, partial) = decide_id(&cache, &chan, &id, asked);
            if decision == shadow::Decision::Serve {
                vouched.push(format!(
                    "{case}, asked {} s on: {decision:?} {content:?} partial {partial:?}",
                    (asked - first) / SEC
                ));
            }
        }
        // A read begun after the drop and the push vouches for the message again.
        let later = first + 25 * HOUR + SEC;
        clock.set(later);
        read_by_id(&cache, &chan, now.clone(), later);
        let asked = later + 5 * SEC;
        clock.set(asked);
        assert_eq!(
            decide_id(&cache, &chan, &id, asked),
            (shadow::Decision::Serve, Some(now.content), Some(false)),
            "{case}: a read begun after the drop vouches for {id} again"
        );
    }
    assert!(vouched.is_empty(), "a push lifted the drop: {vouched:#?}");
}

// ---- the fifth review of the fix: a read begun before an un-timed pushed edit ------------------

/// What the cache would decide for a window of `limit` of a live-fed bridge's channel, asked at
/// `asked_ms`, and the text it holds for m3.
fn decide_window_m3(
    cache: &MessageCache,
    chan: &ChanKey,
    limit: usize,
    asked_ms: i64,
) -> (shadow::Decision, Option<String>) {
    let ask = window(limit);
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
    let m3 = view
        .rows
        .iter()
        .find(|r| r.message.id.0 == "m3")
        .map(|r| r.message.content.clone());
    (shadow::decide(&ask, &view, &facts), m3)
}

/// A bridge whose reads carry edit times, and whose adapter pushes edits without one (the contract
/// lets `edited_timestamp` be absent). A read brings m3 as edited once (`v1`, edited at E0). The
/// adapter pushes an un-timed edit to `v2`. A read begins and sees `v2`, edited at E2. The adapter
/// pushes an un-timed edit to `v3`, and only then does the read land: a read by id, or with
/// `listing` a proven listing of Flat's newest end. An un-timed edit leaves the held edit time at
/// E0, so the read's E2 looked like a later edit than the one held, and its `v2` was stored as the
/// whole message, with the read's older time: nothing recorded the push of `v3`, and the feed
/// vouched for `v2`. The read began before that push, so it must change nothing (rule 5). Returns
/// each answer that serves m3 as anything but `v3`, by id and in a window of 8, 5 s on and a day
/// and an hour on.
fn read_begun_before_an_untimed_edit(listing: bool) -> Vec<String> {
    let dir = crate::testing::TempDir::new("cache-untimed-edit-race");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    let edited = |text: &str, edited_ms: i64| Message {
        content: text.to_owned(),
        edited_timestamp: Some(crate::clock::iso_from_ms(edited_ms)),
        ..m(3)
    };
    clock.set(first + 5 * SEC);
    read_by_id(
        &cache,
        &chan,
        edited("v1 text", first + 2 * SEC),
        first + 5 * SEC,
    );
    clock.set(first + 10 * SEC);
    push_text(&cache, &chan, m(3), "v2 text", true, first + 10 * SEC);
    let read_began = first + 12 * SEC;
    clock.set(read_began);
    let stamp = cache.stamp(&chan);
    clock.set(first + 20 * SEC);
    push_text(&cache, &chan, m(3), "v3 text", true, first + 20 * SEC);
    clock.set(first + 21 * SEC);
    let v2 = edited("v2 text", first + 10 * SEC);
    let observation = if listing {
        Observation::Listing(Listing {
            scope: Scope::Flat,
            rows: (1..=8)
                .map(|n| Incoming::new(if n == 3 { v2.clone() } else { m(n) }, Scope::Flat))
                .collect(),
            observed_ms: read_began,
            proof: Some(newest_proof(ListStart::At(T0 + STEP))),
        })
    } else {
        Observation::Rows {
            rows: vec![Incoming::new(v2, Scope::Flat)],
            scope: None,
            observed_ms: read_began,
        }
    };
    cache.offer(chan.clone(), stamp, observation);
    assert!(cache.quiesce_blocking(WAIT));
    let how = if listing { "a listing" } else { "a read by id" };
    let mut vouched = Vec::new();
    for asked in [first + 26 * SEC, first + 25 * HOUR] {
        clock.set(asked);
        let (decision, content, partial) = decide_id(&cache, &chan, "m3", asked);
        if decision == shadow::Decision::Serve && content.as_deref() != Some("v3 text") {
            vouched.push(format!(
                "{how}, by id, asked {} s on: {decision:?} {content:?} partial {partial:?}",
                (asked - first) / SEC
            ));
        }
        let (decision, content) = decide_window_m3(&cache, &chan, 8, asked);
        if decision == shadow::Decision::Serve && content.as_deref() != Some("v3 text") {
            vouched.push(format!(
                "{how}, a window of 8, asked {} s on: {decision:?} m3 {content:?}",
                (asked - first) / SEC
            ));
        }
    }
    vouched
}

/// The review's probe
/// `r7_a_a_read_begun_before_an_untimed_pushed_edit_never_puts_back_the_version_before_it`.
#[test]
fn a_read_begun_before_an_untimed_pushed_edit_never_puts_back_the_version_before_it() {
    let mut vouched = read_begun_before_an_untimed_edit(false);
    vouched.extend(read_begun_before_an_untimed_edit(true));
    assert!(
        vouched.is_empty(),
        "text edited away vouched for: {vouched:#?}"
    );
}

/// The review's control: the same race with no edit time on the read's copy keeps `v3`, held as
/// possibly a snippet, so never served.
#[test]
fn control_a_read_begun_before_an_untimed_pushed_edit_without_an_edit_time_keeps_the_pushed_text() {
    let dir = crate::testing::TempDir::new("cache-untimed-edit-race-control");
    let first = T0 + 8 * STEP + SEC;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let (cache, chan) = bridge_with_flat(&dir, &clock, first);
    clock.set(first + 10 * SEC);
    push_text(&cache, &chan, m(3), "v2 text", true, first + 10 * SEC);
    let read_began = first + 12 * SEC;
    clock.set(read_began);
    let stamp = cache.stamp(&chan);
    clock.set(first + 20 * SEC);
    push_text(&cache, &chan, m(3), "v3 text", true, first + 20 * SEC);
    clock.set(first + 21 * SEC);
    cache.offer(
        chan.clone(),
        stamp,
        Observation::Rows {
            rows: vec![Incoming::new(
                Message {
                    content: "v2 text".to_owned(),
                    ..m(3)
                },
                Scope::Flat,
            )],
            scope: None,
            observed_ms: read_began,
        },
    );
    assert!(cache.quiesce_blocking(WAIT));
    let asked = first + 26 * SEC;
    clock.set(asked);
    let (decision, content, partial) = decide_id(&cache, &chan, "m3", asked);
    assert_eq!(content.as_deref(), Some("v3 text"));
    assert_eq!(partial, Some(true));
    assert_ne!(decision, shadow::Decision::Serve);
}

/// The same root, by a read that carries no edit time (a bridge's copy may come by a path that
/// cannot say). Proven whole by such a read, begun at 15 s, the pushed `v3` is served, so the read
/// begun before the push that brings `v2`, edited at E2, must still change nothing. And with no push
/// at all: such a read, begun at 30 s, brings `v3` over `v1`, and a read begun at 25 s lands with
/// `v2`, edited at E2. E2 is later than `v1`'s E0, but not therefore than `v3`, whose edit time
/// nothing said. Asked 5 s on, and a day and an hour on.
#[test]
fn a_read_that_cannot_say_when_its_text_was_edited_never_lets_an_earlier_read_put_back_an_older_one(
) {
    let mut vouched = Vec::new();
    for case in ["a pushed text proven whole", "a read's text replaced"] {
        let dir = crate::testing::TempDir::new("cache-untimed-read");
        let first = T0 + 8 * STEP + SEC;
        let clock = Arc::new(ManualClock(AtomicI64::new(first)));
        let (cache, chan) = bridge_with_flat(&dir, &clock, first);
        let edited = |text: &str, edited_ms: i64| Message {
            content: text.to_owned(),
            edited_timestamp: Some(crate::clock::iso_from_ms(edited_ms)),
            ..m(3)
        };
        let untimed_v3 = Message {
            content: "v3 text".to_owned(),
            ..m(3)
        };
        clock.set(first + 5 * SEC);
        read_by_id(
            &cache,
            &chan,
            edited("v1 text", first + 2 * SEC),
            first + 5 * SEC,
        );
        let read_began = if case == "a pushed text proven whole" {
            clock.set(first + 10 * SEC);
            push_text(&cache, &chan, m(3), "v2 text", true, first + 10 * SEC);
            first + 12 * SEC
        } else {
            first + 25 * SEC
        };
        clock.set(read_began);
        let stamp = cache.stamp(&chan);
        if case == "a pushed text proven whole" {
            clock.set(first + 20 * SEC);
            push_text(&cache, &chan, m(3), "v3 text", true, first + 20 * SEC);
            clock.set(first + 21 * SEC);
            read_by_id(&cache, &chan, untimed_v3, first + 15 * SEC);
        } else {
            clock.set(first + 31 * SEC);
            read_by_id(&cache, &chan, untimed_v3, first + 30 * SEC);
        }
        let asked = first + 32 * SEC;
        clock.set(asked);
        assert_eq!(
            decide_id(&cache, &chan, "m3", asked),
            (
                shadow::Decision::Serve,
                Some("v3 text".to_owned()),
                Some(false)
            ),
            "{case}: the control, v3 held whole"
        );
        cache.offer(
            chan.clone(),
            stamp,
            Observation::Rows {
                rows: vec![Incoming::new(
                    edited("v2 text", first + 10 * SEC),
                    Scope::Flat,
                )],
                scope: None,
                observed_ms: read_began,
            },
        );
        assert!(cache.quiesce_blocking(WAIT));
        for asked in [first + 37 * SEC, first + 25 * HOUR] {
            clock.set(asked);
            let (decision, content, partial) = decide_id(&cache, &chan, "m3", asked);
            if decision == shadow::Decision::Serve && content.as_deref() != Some("v3 text") {
                vouched.push(format!(
                    "{case}, asked {} s on: {decision:?} {content:?} partial {partial:?}",
                    (asked - first) / SEC
                ));
            }
        }
    }
    assert!(
        vouched.is_empty(),
        "an older text vouched for: {vouched:#?}"
    );
}
