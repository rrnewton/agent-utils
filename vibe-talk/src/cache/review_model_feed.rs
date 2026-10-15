//! Review round 1: a model check of a live-fed bridge's pushed edits and deletions racing scope
//! erases, listings and the writer (scratch only). Every upstream change is pushed; the cache must
//! never serve, by id, a message whose deletion it was pushed, nor text no version upstream had
//! as of what it was pushed.

use std::collections::HashMap;
use std::sync::atomic::AtomicI64;
use std::sync::{mpsc, Arc};

use super::fixtures::{self, newest_proof, ManualClock, T0};
use super::tests::{m, STEP};
use super::*;

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

type State = HashMap<String, Option<String>>;
/// Every version each message has had upstream, oldest first, and how many of them were pushed.
type History = HashMap<String, (Vec<Option<String>>, usize)>;

fn run(seed: u64) -> Vec<String> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let dir = crate::testing::TempDir::new("review-model-feed");
    let first = T0 + 8 * STEP + 1_000;
    let clock = Arc::new(ManualClock(AtomicI64::new(first)));
    let chan = ChanKey::new("p", "C1");
    let cache = fixtures::open(&dir, &clock, Limits::default());
    cache.retain_only(RouteSnapshot::new([chan.clone()], ["p".to_owned()]));
    assert!(cache.quiesce_blocking(std::time::Duration::from_secs(30)));
    let mut truth: State = (1..=8)
        .map(|n| (format!("m{n}"), Some(format!("body number {n}"))))
        .collect();
    let listing = |state: &State, observed_ms: i64| {
        let rows = (1..=8)
            .filter_map(|n| {
                let id = format!("m{n}");
                state.get(&id).cloned().flatten().map(|text| {
                    let mut msg = m(n);
                    msg.content = text;
                    Incoming::new(msg, Scope::Flat)
                })
            })
            .collect();
        Observation::Listing(Listing {
            scope: Scope::Flat,
            rows,
            observed_ms,
            proof: Some(newest_proof(ListStart::At(T0 + STEP))),
        })
    };
    let settle = |cache: &MessageCache| {
        assert!(cache.quiesce_blocking(std::time::Duration::from_secs(30)));
    };
    let stamp = cache.stamp(&chan);
    cache.offer(chan.clone(), stamp, listing(&truth, first));
    settle(&cache);
    // What the pushes offered so far say.
    let mut history: History = truth.iter().map(|(k, v)| (k.clone(), (vec![v.clone()], 1))).collect();
    let no_ties = std::env::var("REVIEW_NO_TIES").is_ok();
    let mut pending: Vec<(Stamp, i64, String, Option<String>)> = Vec::new();
    let mut inflight: Vec<(Stamp, i64, State)> = Vec::new();
    let mut paused: Option<mpsc::Sender<()>> = None;
    let mut trace: Vec<String> = Vec::new();
    let mut bad = Vec::new();
    let mut now = first;
    let mut edits = 0;
    for _ in 0..24 {
        now += [0, 0, 1, 5, 20, 500, 3_000][rng.below(7) as usize];
        let op = rng.below(10);
        if no_ties && op <= 1 {
            now += 1;
        }
        clock.set(now);
        match op {
            0 => {
                let n = 1 + rng.below(8);
                let id = format!("m{n}");
                if truth[&id].is_some() {
                    edits += 1;
                    let text = format!("edit {edits} of {n}");
                    truth.insert(id.clone(), Some(text.clone()));
                    history.get_mut(&id).unwrap().0.push(Some(text.clone()));
                    pending.push((cache.stamp(&chan), now, id.clone(), Some(text)));
                    trace.push(format!("edit {id} @{}", now - first));
                }
            }
            1 => {
                let n = 1 + rng.below(8);
                let id = format!("m{n}");
                if truth[&id].is_some() {
                    truth.insert(id.clone(), None);
                    history.get_mut(&id).unwrap().0.push(None);
                    pending.push((cache.stamp(&chan), now, id.clone(), None));
                    trace.push(format!("delete {id} @{}", now - first));
                }
            }
            2 | 3 if !pending.is_empty() => {
                let (stamp, observed, id, change) = pending.remove(0);
                let push = match &change {
                    Some(text) => {
                        let n: i64 = id[1..].parse().unwrap();
                        let mut msg = m(n);
                        msg.content.clone_from(text);
                        Push::Upsert {
                            message: Box::new(Incoming::new(msg, Scope::Flat)),
                            update: true,
                            partial: false,
                        }
                    }
                    None => Push::Delete {
                        id: MessageId(id.clone()),
                        created_ms: None,
                    },
                };
                cache.offer(
                    chan.clone(),
                    stamp,
                    Observation::Push {
                        push,
                        observed_ms: observed,
                    },
                );
                drop(change);
                history.get_mut(&id).unwrap().1 += 1;
                trace.push(format!("push {id} of @{} at @{}", observed - first, now - first));
            }
            4 => {
                inflight.push((cache.stamp(&chan), now, truth.clone()));
                trace.push(format!("start listing @{}", now - first));
            }
            5 if !inflight.is_empty() => {
                let i = rng.below(inflight.len() as u64) as usize;
                let (stamp, observed, seen) = inflight.remove(i);
                cache.offer(chan.clone(), stamp, listing(&seen, observed));
                trace.push(format!("land listing of @{} at @{}", observed - first, now - first));
            }
            6 => {
                let thread = ["t9", "1700000000.999999"][rng.below(2) as usize];
                cache.erase_scope(&chan, &Scope::Thread(thread.to_owned()));
                trace.push(format!("erase {thread} @{}", now - first));
            }
            7 => {
                if let Some(resume) = paused.take() {
                    drop(resume);
                    trace.push(format!("resume @{}", now - first));
                } else {
                    paused = Some(cache.pause_writer());
                    trace.push(format!("pause @{}", now - first));
                }
            }
            _ => {
                if paused.is_none() {
                    settle(&cache);
                    for later in [now, now + 5_000] {
                        clock.set(later.max(now));
                        bad.extend(check(&cache, &chan, later, &history, &trace));
                    }
                    clock.set(now);
                }
            }
        }
    }
    bad
}

fn check(
    cache: &MessageCache,
    chan: &ChanKey,
    asked_ms: i64,
    history: &History,
    trace: &[String],
) -> Vec<String> {
    let mut bad = Vec::new();
    for n in 1..=8 {
        let id = format!("m{n}");
        let ask = shadow::Ask::Message {
            consumer: shadow::Consumer::MessageById,
            id: MessageId(id.clone()),
        };
        let query = shadow::query(&ask, observe::Shape::Bridge, true).expect("a query");
        let Some(view) = cache.shadow_view(chan, &query).served() else {
            continue;
        };
        let facts = shadow::Facts {
            asked_ms,
            live_fed: true,
            fed_since_ms: Some(cache.fed_since_ms()),
            dropped_ms: cache.last_drop_ms(chan),
            barrier: true,
            feed: shadow::Feed::Deletions,
        };
        if shadow::decide(&ask, &view, &facts) != shadow::Decision::Serve {
            continue;
        }
        let text = view.rows.first().map(|r| r.message.content.clone());
        let (versions, pushed) = &history[&id];
        // Any version from the latest pushed on, and never once its deletion was pushed.
        let allowed = &versions[pushed - 1..];
        let ok = text.is_some() && allowed.contains(&text);
        if !ok {
            bad.push(format!(
                "{id}: served {text:?}; versions {versions:?}, {pushed} pushed\n  trace {trace:?}"
            ));
        }
    }
    bad
}

#[test]
fn review_model_feed_against_scope_erases() {
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
