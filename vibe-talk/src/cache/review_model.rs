//! Review round 1: a small model check of erase_scope racing listings (scratch only).
//!
//! Upstream never changes. The cache erases thread m1 (a bogus refusal, confirmed), which tombstones
//! r1 (a reply also listed in Main), r2 and r3, while reads of Main and of the thread start and
//! land around the erase's call and its apply. Whatever the interleaving, any decision to serve
//! must hand back exactly what upstream lists.

use std::sync::mpsc;

use super::fixtures::{message, reply, Fixture, T0};
use super::tests::{m, STEP};
use super::*;
use crate::model::Message;

const HOUR: i64 = 3_600_000;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    MainNewest,
    MainSmall,
    MainRange(i64, i64),
    Thread,
}

struct World {
    root: Message,
    main: Vec<(Message, Scope)>,
    thread: Vec<(Message, Scope)>,
}

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

fn world(r1_ms: i64, r3_ms: i64) -> World {
    let t = Scope::Thread("m1".to_owned());
    let root = root("m1", T0 + STEP, 3);
    let r1 = reply("r1", "m1", "also in the channel", r1_ms);
    let r2 = reply("r2", "m1", "two", T0 + STEP + 10_000);
    let r3 = reply("r3", "m1", "three", r3_ms);
    let mut main = vec![(root.clone(), Scope::Main)];
    main.extend((2..=6).map(|n| (m(n), Scope::Main)));
    main.push((r1.clone(), t.clone()));
    main.sort_by_key(|(msg, _)| rows::created_ms(msg));
    let mut thread = vec![
        (root.clone(), Scope::Main),
        (r1, t.clone()),
        (r2, t.clone()),
        (r3, t),
    ];
    thread.sort_by_key(|(msg, _)| rows::created_ms(msg));
    World { root, main, thread }
}

fn proof(from: i64, until: ListEnd) -> Proof {
    Proof {
        from: ListStart::At(from),
        until,
        complete: true,
        full: true,
        source: coverage::Source::Read,
    }
}

fn listing(w: &World, kind: Kind, observed_ms: i64) -> Observation {
    let ms = |msg: &Message| rows::created_ms(msg).unwrap();
    let (scope, rows, proof) = match kind {
        Kind::MainNewest => (
            Scope::Main,
            w.main.clone(),
            proof(T0 + STEP, ListEnd::Newest),
        ),
        Kind::MainSmall => {
            let rows: Vec<_> = w.main[w.main.len() - 2..].to_vec();
            let from = ms(&rows[0].0);
            (Scope::Main, rows, proof(from, ListEnd::Newest))
        }
        Kind::MainRange(a, b) => (
            Scope::Main,
            w.main
                .iter()
                .filter(|(msg, _)| (a..b).contains(&ms(msg)))
                .cloned()
                .collect(),
            proof(a, ListEnd::Before(b)),
        ),
        Kind::Thread => (
            Scope::Thread("m1".to_owned()),
            w.thread.clone(),
            proof(ms(&w.root), ListEnd::Newest),
        ),
    };
    Observation::Listing(Listing {
        scope,
        rows: rows
            .into_iter()
            .map(|(msg, home)| Incoming::new(msg, home))
            .collect(),
        observed_ms,
        proof: Some(proof),
    })
}

fn check(fx: &Fixture, w: &World, asked_ms: i64, trace: &[String]) -> Vec<String> {
    let mut bad = Vec::new();
    let ms = |msg: &Message| rows::created_ms(msg).unwrap();
    let ids = |list: &[(Message, Scope)]| -> Vec<String> {
        list.iter().map(|(m, _)| m.id.0.clone()).collect()
    };
    let facts = shadow::Facts {
        asked_ms,
        live_fed: false,
        fed_since_ms: Some(fx.cache.fed_since_ms()),
        dropped_ms: fx.cache.last_drop_ms(&fx.chan),
        barrier: true,
        feed: shadow::Feed::AdditionsOnly,
    };
    let mut asks: Vec<(shadow::Ask, Vec<String>)> = Vec::new();
    for k in 1..=8usize {
        let main = ids(&w.main);
        let want = main[main.len().saturating_sub(k)..].to_vec();
        asks.push((
            shadow::Ask::Window {
                consumer: shadow::Consumer::Messages,
                limit: k,
            },
            want.clone(),
        ));
        let th = ids(&w.thread);
        asks.push((
            shadow::Ask::Timeline {
                view: crate::threads::TimelineView::Thread,
                thread: Some("m1".to_owned()),
                limit: k,
            },
            th[th.len().saturating_sub(k)..].to_vec(),
        ));
    }
    let instants: Vec<i64> = w.main.iter().map(|(m, _)| ms(m)).collect();
    for &since in &instants {
        for &until in &instants {
            if until <= since {
                continue;
            }
            for limit in [1usize, 2, 10] {
                let inside: Vec<String> = w
                    .main
                    .iter()
                    .filter(|(m, _)| (since..until).contains(&ms(m)))
                    .map(|(m, _)| m.id.0.clone())
                    .take(limit + 1)
                    .collect();
                asks.push((
                    shadow::Ask::Page {
                        since_ms: since,
                        until_ms: Some(until),
                        limit,
                    },
                    inside,
                ));
            }
        }
        let after: Vec<String> = w
            .main
            .iter()
            .filter(|(m, _)| ms(m) >= since)
            .map(|(m, _)| m.id.0.clone())
            .collect();
        asks.push((
            shadow::Ask::Count {
                since_ms: Some(since),
                cap: 100,
            },
            after,
        ));
    }
    for (ask, want) in asks {
        let Some(query) = shadow::query(&ask, observe::Shape::DiscordNative, false) else {
            continue;
        };
        let Some(view) = fx.cache.shadow_view(&fx.chan, &query).served() else {
            continue;
        };
        let d = shadow::decide(&ask, &view, &facts);
        if d != shadow::Decision::Serve {
            continue;
        }
        let got: Vec<String> = view.rows.iter().map(|r| r.message.id.0.clone()).collect();
        let got = match &ask {
            shadow::Ask::Window { limit, .. } | shadow::Ask::Timeline { limit, .. } => {
                got[got.len().saturating_sub(*limit)..].to_vec()
            }
            _ => got,
        };
        if got != want {
            bad.push(format!(
                "asked at +{}: {ask:?} served {got:?}, upstream {want:?}\n  trace: {trace:?}",
                asked_ms - T0 - 6 * STEP
            ));
        }
    }
    bad
}

fn run(seed: u64) -> Vec<String> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let r1_ms = if rng.below(2) == 0 {
        T0 + 3 * STEP + 5
    } else {
        T0 + 6 * STEP - 30_000
    };
    let r3_ms = if rng.below(2) == 0 {
        T0 + 4 * STEP
    } else {
        T0 + 6 * STEP - 20_000
    };
    let w = world(r1_ms, r3_ms);
    let fx = Fixture::new("review-model");
    let first = T0 + 6 * STEP + 1_000;
    fx.clock.set(first);
    fx.offer(&fx.chan, listing(&w, Kind::MainNewest, first));
    fx.offer(&fx.chan, listing(&w, Kind::Thread, first));
    let mut trace = vec![format!("r1 {} r3 {}", r1_ms - T0, r3_ms - T0)];
    let mut inflight: Vec<(Stamp, i64, Kind)> = Vec::new();
    let mut paused: Option<mpsc::Sender<()>> = None;
    let mut erases = 0;
    let mut bad = Vec::new();
    let mut now = first;
    for _ in 0..16 {
        now += [0, 0, 1, 5, 20, 500, 3_000][rng.below(7) as usize];
        fx.clock.set(now);
        match rng.below(8) {
            0 | 1 => {
                let kind = match rng.below(4) {
                    0 => Kind::MainNewest,
                    1 => Kind::MainSmall,
                    2 => {
                        let a = T0 + (1 + rng.below(5) as i64) * STEP;
                        Kind::MainRange(a, a + (1 + rng.below(3) as i64) * STEP)
                    }
                    _ => Kind::Thread,
                };
                // Sometimes the read's start is dated before its stamp (a snapshot's as_of).
                let observed = now - [0, 0, 0, 10][rng.below(4) as usize];
                inflight.push((fx.cache.stamp(&fx.chan), observed, kind));
                trace.push(format!("start {kind:?} @{}", observed - first));
            }
            2 | 3 if !inflight.is_empty() => {
                let i = rng.below(inflight.len() as u64) as usize;
                let (stamp, observed, kind) = inflight.remove(i);
                fx.cache
                    .offer(fx.chan.clone(), stamp, listing(&w, kind, observed));
                trace.push(format!("land {kind:?} of @{} at @{}", observed - first, now - first));
            }
            4 if erases < 2 => {
                fx.cache
                    .erase_scope(&fx.chan, &Scope::Thread("m1".to_owned()));
                erases += 1;
                trace.push(format!("erase @{}", now - first));
            }
            5 => {
                if let Some(resume) = paused.take() {
                    drop(resume);
                    trace.push(format!("resume @{}", now - first));
                } else {
                    paused = Some(fx.cache.pause_writer());
                    trace.push(format!("pause @{}", now - first));
                }
            }
            _ => {
                if paused.is_none() {
                    fx.settle();
                    for later in [now, now + 3_000, now + HOUR] {
                        bad.extend(check(&fx, &w, later, &trace));
                    }
                }
            }
        }
    }
    if let Some(resume) = paused.take() {
        drop(resume);
    }
    fx.settle();
    // Land whatever is still out, then decide.
    for (stamp, observed, kind) in inflight.drain(..) {
        fx.cache
            .offer(fx.chan.clone(), stamp, listing(&w, kind, observed));
        trace.push(format!("land {kind:?} of @{} at end", observed - first));
    }
    fx.settle();
    for later in [now, now + 3_000, now + HOUR] {
        bad.extend(check(&fx, &w, later, &trace));
    }
    bad
}

#[test]
fn review_model_erase_scope_racing_listings() {
    let seeds: u64 = std::env::var("REVIEW_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let mut failures = Vec::new();
    for seed in 0..seeds {
        let bad = run(seed);
        if !bad.is_empty() {
            failures.push(format!("seed {seed}: {}", bad[0]));
        }
    }
    eprintln!("{} of {seeds} seeds failed", failures.len());
    for f in failures.iter().take(8) {
        eprintln!("{f}");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
}
