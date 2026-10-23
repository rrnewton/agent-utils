//! The replication model's §8 on the server's message cache: random interleavings of what the
//! provider does, what its live feed delivers and when, and reads that start and land out of
//! order (some answered from a memory that says how old it is), checked against a reference model
//! of the provider after every step. `#469 replication-model-tests`.
//!
//! `ai_docs/transient/2026-10-23-replication-model.md` §8. The reference model is the provider:
//! every message with every version it has had and when, and when it was deleted. The cache is
//! the real one, through its real writer. A failure prints its seed and the steps that led to it,
//! so it replays exactly; shrink by hand.
//!
//! The properties, numbered as §8.3 numbers them:
//!
//! - **5, no resurrection.** Once the live feed's deletion of a message has been applied, the
//!   message is never served again: the provider never brings an id back.
//! - **6, no reversion.** A message's served text never goes from a later version back to an
//!   earlier one, whatever reads land late or answer from memory.
//! - **Nothing invented.** A served text is one the provider really gave the message.
//! - **Convergence.** Once the feed is delivered, every read has landed and two reads of the newest
//!   end far enough apart have settled the unsettled band, the cache serves exactly the provider's
//!   messages, each at its current version.

use std::collections::{BTreeMap, BTreeSet};

use super::fixtures::{message, newest_proof, Fixture, T0};
use super::*;
use crate::model::Message;

const SEC: i64 = 1_000;
const MINUTE: i64 = 60 * SEC;
/// Steps per seed, and seeds per run: enough interleavings to reach the races §5.4 names,
/// small enough for the gate.
const STEPS: usize = 150;
const SEEDS: u64 = 48;

/// xorshift64*: plenty for choosing events, and replayable from a seed with no dependency.
struct Steps(u64);

impl Steps {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn pick<T: Clone>(&mut self, from: &[T]) -> Option<T> {
        if from.is_empty() {
            None
        } else {
            Some(from[usize::try_from(self.below(from.len() as u64)).unwrap_or(0)].clone())
        }
    }
}

/// One message as the provider holds it: every version with when it was made, and when it was
/// deleted.
#[derive(Clone, Debug)]
struct Upstream {
    id: String,
    created_ms: i64,
    /// `(made at, text, edit time)`, oldest first; the first is the creation, with no edit time.
    versions: Vec<(i64, String, Option<i64>)>,
    deleted_ms: Option<i64>,
}

impl Upstream {
    /// The version current at `at`, if the message existed then.
    fn at(&self, at: i64) -> Option<usize> {
        if self.created_ms > at || self.deleted_ms.is_some_and(|gone| gone <= at) {
            return None;
        }
        self.versions.iter().rposition(|(made, _, _)| *made <= at)
    }

    fn copy(&self, version: usize) -> Message {
        let (_, text, edited) = &self.versions[version];
        let mut copy = message(&self.id, "ada", text, self.created_ms);
        copy.edited_timestamp = edited.map(crate::clock::iso_from_ms);
        copy
    }

    /// Which version a served text is, if it is one.
    fn version_of(&self, text: &str) -> Option<usize> {
        self.versions.iter().position(|(_, held, _)| held == text)
    }
}

/// A change the live feed has yet to deliver.
#[derive(Clone, Debug)]
enum Pending {
    Upsert {
        id: String,
        version: usize,
        edit_time: bool,
    },
    Delete {
        id: String,
    },
}

/// A read begun and not landed: what it saw, and the instant it says it saw it at.
#[derive(Clone, Debug)]
struct InFlight {
    observed_ms: i64,
    rows: Vec<Message>,
    oldest_ms: i64,
}

struct World {
    fx: Fixture,
    now: i64,
    next_id: u32,
    upstream: BTreeMap<String, Upstream>,
    feed: Vec<Pending>,
    reads: Vec<InFlight>,
    /// Ids whose deletion the feed has delivered: property 5.
    deleted_delivered: BTreeSet<String>,
    /// The highest version each id has been served at: property 6.
    served_at: BTreeMap<String, usize>,
    log: Vec<String>,
}

impl World {
    fn new(tag: &str) -> Self {
        let fx = Fixture::new(tag);
        fx.clock.set(T0);
        // The route snapshot's sweep is applied before anything is read.
        fx.settle();
        Self {
            fx,
            now: T0,
            next_id: 0,
            upstream: BTreeMap::new(),
            feed: Vec::new(),
            reads: Vec::new(),
            deleted_delivered: BTreeSet::new(),
            served_at: BTreeMap::new(),
            log: Vec::new(),
        }
    }

    fn tick(&mut self, ms: i64) {
        self.now += ms.max(1);
        self.fx.clock.set(self.now);
    }

    fn live(&self) -> Vec<String> {
        self.upstream
            .values()
            .filter(|m| m.deleted_ms.is_none())
            .map(|m| m.id.clone())
            .collect()
    }

    fn create(&mut self, edit_time: bool) {
        self.next_id += 1;
        let id = format!("m{:04}", self.next_id);
        self.upstream.insert(
            id.clone(),
            Upstream {
                id: id.clone(),
                created_ms: self.now,
                versions: vec![(self.now, format!("{id} v0"), None)],
                deleted_ms: None,
            },
        );
        self.log.push(format!("{} create {id}", self.now));
        self.feed.push(Pending::Upsert {
            id,
            version: 0,
            edit_time,
        });
    }

    fn edit(&mut self, id: &str, edit_time: bool) {
        let now = self.now;
        let held = self.upstream.get_mut(id).expect("a live message");
        let version = held.versions.len();
        held.versions
            .push((now, format!("{id} v{version}"), Some(now)));
        self.log.push(format!("{now} edit {id} to v{version}"));
        self.feed.push(Pending::Upsert {
            id: id.to_owned(),
            version,
            edit_time,
        });
    }

    fn delete(&mut self, id: &str) {
        self.upstream
            .get_mut(id)
            .expect("a live message")
            .deleted_ms = Some(self.now);
        self.log.push(format!("{} delete {id}", self.now));
        self.feed.push(Pending::Delete { id: id.to_owned() });
    }

    /// Deliver the feed's change at `index`, out of order with the rest.
    fn deliver(&mut self, index: usize) {
        let change = self.feed.remove(index);
        let push = match &change {
            Pending::Upsert {
                id,
                version,
                edit_time,
            } => {
                let mut copy = self.upstream[id].copy(*version);
                if !edit_time {
                    // A path that cannot say when the message was edited.
                    copy.edited_timestamp = None;
                }
                Push::Upsert {
                    message: Box::new(Incoming::new(copy, Scope::Main)),
                    update: *version > 0,
                    partial: false,
                }
            }
            Pending::Delete { id } => Push::Delete {
                id: MessageId(id.clone()),
                created_ms: Some(self.upstream[id].created_ms),
            },
        };
        self.log.push(format!("{} deliver {change:?}", self.now));
        let chan = self.fx.chan.clone();
        self.fx.offer(
            &chan,
            Observation::Push {
                push,
                observed_ms: self.now,
            },
        );
        if let Pending::Delete { id } = change {
            self.deleted_delivered.insert(id);
        }
    }

    /// Begin a read of the newest end, answered as the provider stood `age_ms` ago and saying so.
    fn begin_read(&mut self, age_ms: i64) {
        let at = self.now - age_ms;
        let rows: Vec<Message> = self
            .upstream
            .values()
            .filter_map(|m| m.at(at).map(|version| m.copy(version)))
            .collect();
        self.log.push(format!(
            "{} begin read as of {at}: {} rows",
            self.now,
            rows.len()
        ));
        self.reads.push(InFlight {
            observed_ms: at,
            rows,
            oldest_ms: T0,
        });
    }

    /// Land the read at `index`, late or in order.
    fn land_read(&mut self, index: usize) {
        let read = self.reads.remove(index);
        self.log.push(format!(
            "{} land read observed {}",
            self.now, read.observed_ms
        ));
        let chan = self.fx.chan.clone();
        self.fx.offer(
            &chan,
            Observation::Listing(Listing {
                scope: Scope::Main,
                rows: read
                    .rows
                    .into_iter()
                    .map(|m| Incoming::new(m, Scope::Main).reporting_edits(true))
                    .collect(),
                observed_ms: read.observed_ms,
                proof: Some(newest_proof(ListStart::At(read.oldest_ms))),
            }),
        );
    }

    fn served(&self) -> Vec<CachedMessage> {
        match self.fx.cache.newest(&self.fx.chan, &Scope::Main, 10_000) {
            Read::Served(rows) => rows,
            other => panic!(
                "not served: {}\nsteps:\n  {}",
                match other {
                    Read::Off => "off".to_owned(),
                    Read::Moved => "moved".to_owned(),
                    Read::Blocked => "blocked".to_owned(),
                    Read::Failed(code) => code,
                    Read::Served(_) => unreachable!(),
                },
                self.log.join("\n  ")
            ),
        }
    }

    fn fail(&self, seed: u64, what: &str) -> ! {
        panic!("seed {seed}: {what}\nsteps:\n  {}", self.log.join("\n  "));
    }

    /// Properties 5 and 6 and nothing invented, after every step.
    fn check(&mut self, seed: u64) {
        for row in self.served() {
            let id = row.message.id.0.clone();
            if self.deleted_delivered.contains(&id) {
                self.fail(
                    seed,
                    &format!("{id} served after its deletion was delivered"),
                );
            }
            let Some(held) = self.upstream.get(&id) else {
                self.fail(seed, &format!("{id} served but never created"));
            };
            let Some(version) = held.version_of(&row.message.content) else {
                self.fail(
                    seed,
                    &format!(
                        "{id} served with text it never had: {:?}",
                        row.message.content
                    ),
                );
            };
            let best = self.served_at.entry(id.clone()).or_insert(version);
            if version < *best {
                let was = *best;
                self.fail(seed, &format!("{id} went back from v{was} to v{version}"));
            }
            *best = version;
        }
    }

    /// Deliver everything, land everything, then two reads of the newest end far enough apart
    /// to settle the band, and compare with the provider.
    fn quiesce_and_compare(&mut self, seed: u64) {
        while !self.feed.is_empty() {
            self.deliver(0);
            self.check(seed);
        }
        while !self.reads.is_empty() {
            self.land_read(0);
            self.check(seed);
        }
        for _ in 0..2 {
            self.tick(coverage::SETTLE_AFTER_LOCAL_MS + coverage::SETTLE_MS + SEC);
            self.begin_read(0);
            self.land_read(0);
            self.check(seed);
        }
        let served: BTreeMap<String, String> = self
            .served()
            .into_iter()
            .map(|row| (row.message.id.0, row.message.content))
            .collect();
        let expected: BTreeMap<String, String> = self
            .upstream
            .values()
            .filter(|m| m.deleted_ms.is_none())
            .map(|m| {
                (
                    m.id.clone(),
                    m.versions.last().expect("a version").1.clone(),
                )
            })
            .collect();
        if served != expected {
            self.fail(
                seed,
                &format!("did not converge:\n  served   {served:?}\n  provider {expected:?}"),
            );
        }
    }
}

fn run(seed: u64) {
    let mut world = World::new(&format!("cache-model-{seed}"));
    let mut steps = Steps::new(seed);
    for _ in 0..STEPS {
        world.tick(i64::try_from(steps.below(30)).unwrap_or(0) * SEC + 1);
        match steps.below(100) {
            0..=19 => world.create(steps.below(2) == 0),
            20..=29 => {
                // An edit is always pushed with its edit time, as Slack's tail pushes it: one
                // pushed without (a bridge that cannot say) is not ordered against a creation
                // the feed delivers after it, by anything the cache holds (merge rule 2), so
                // property 6 is not promised for it.
                if let Some(id) = steps.pick(&world.live()) {
                    world.edit(&id, true);
                }
            }
            30..=37 => {
                if let Some(id) = steps.pick(&world.live()) {
                    world.delete(&id);
                }
            }
            38..=59 => {
                if !world.feed.is_empty() {
                    let index = usize::try_from(steps.below(world.feed.len() as u64)).unwrap_or(0);
                    world.deliver(index);
                }
            }
            60..=74 => {
                // Mostly fresh reads; some answered from a memory up to 15 minutes old, which
                // says how old it is (`FLAT_RECONCILE`).
                let age = if steps.below(3) == 0 {
                    i64::try_from(steps.below(15)).unwrap_or(0) * MINUTE
                } else {
                    0
                };
                world.begin_read(age);
            }
            _ => {
                if !world.reads.is_empty() {
                    let index = usize::try_from(steps.below(world.reads.len() as u64)).unwrap_or(0);
                    world.land_read(index);
                }
            }
        }
        world.check(seed);
    }
    world.quiesce_and_compare(seed);
}

#[test]
fn the_cache_agrees_with_the_provider_model_under_random_interleavings() {
    for seed in 1..=SEEDS {
        run(seed);
    }
}

/// C8: a reply the cache did not hold raises its held root's count of none to one, not exact,
/// so a root served from the cache beside an own reply is not drawn unanswered. A later copy of
/// the root with the provider's count replaces it.
#[test]
fn a_new_reply_counts_on_its_held_root() {
    let fx = Fixture::new("c8-root-count");
    let mut root = message("1700000000.000100", "ada", "question", T0);
    root.thread = Some(crate::threads::MessageThread {
        hue: None,
        id: "1700000000.000100".to_owned(),
        root_message_id: Some(MessageId("1700000000.000100".to_owned())),
        is_root: true,
        reply_count: Some(0),
        reply_count_exact: true,
    });
    fx.clock.set(T0 + MINUTE);
    fx.list(
        vec![root.clone()],
        T0 + MINUTE,
        Some(newest_proof(ListStart::At(T0))),
    );
    let count = |fx: &Fixture| {
        fx.cache
            .message(&fx.chan, &MessageId("1700000000.000100".to_owned()))
            .served()
            .expect("served")
            .expect("held")
            .message
            .thread
            .expect("a thread record")
    };
    assert_eq!(count(&fx).reply_count, Some(0));
    // The server's own reply, as `observe::posted` offers it.
    let reply = super::fixtures::reply("r1", "1700000000.000100", "answer", T0 + 2 * MINUTE);
    fx.clock.set(T0 + 2 * MINUTE);
    let chan = fx.chan.clone();
    fx.offer(
        &chan,
        Observation::Rows {
            rows: vec![Incoming::new(
                reply.clone(),
                Scope::Thread("1700000000.000100".to_owned()),
            )],
            scope: Some(Scope::Thread("1700000000.000100".to_owned())),
            observed_ms: T0 + 2 * MINUTE,
        },
    );
    let raised = count(&fx);
    assert_eq!(raised.reply_count, Some(1));
    assert!(
        !raised.reply_count_exact,
        "a lower bound, not the provider's count"
    );
    // The same reply again raises nothing further.
    fx.offer(
        &chan,
        Observation::Rows {
            rows: vec![Incoming::new(
                reply,
                Scope::Thread("1700000000.000100".to_owned()),
            )],
            scope: Some(Scope::Thread("1700000000.000100".to_owned())),
            observed_ms: T0 + 3 * MINUTE,
        },
    );
    assert_eq!(count(&fx).reply_count, Some(1));
    // A read that began before the reply was posted lands after it, with the root's count of
    // none: the reply the file holds keeps the root at one.
    fx.list(
        vec![root.clone()],
        T0 + MINUTE + 30 * SEC,
        Some(newest_proof(ListStart::At(T0))),
    );
    let raced = count(&fx);
    assert_eq!(
        raced.reply_count,
        Some(1),
        "a racing read drew the root unanswered"
    );
    assert!(!raced.reply_count_exact);
    // A newer copy of the root with the provider's own count replaces the bound.
    let mut fresh = root;
    if let Some(thread) = fresh.thread.as_mut() {
        thread.reply_count = Some(2);
    }
    fx.list(
        vec![fresh],
        T0 + 4 * MINUTE,
        Some(newest_proof(ListStart::At(T0))),
    );
    let exact = count(&fx);
    assert_eq!(exact.reply_count, Some(2));
    assert!(exact.reply_count_exact);
}

/// The operation key a provider carried on a message is stored with it, and a later copy that
/// cannot see it (a push, a path that drops metadata) never takes it away. `#465 replication-c6-op-key-marker`.
#[test]
fn the_op_key_is_kept_and_never_erased_by_a_copy_without_it() {
    let fx = Fixture::new("op-key-kept");
    let mut keyed = message("m1", "vibe-talk", "on it", T0);
    keyed.op_key = Some("out-1.00000000deadbeef.0".to_owned());
    fx.clock.set(T0 + MINUTE);
    fx.list(
        vec![keyed],
        T0 + MINUTE,
        Some(newest_proof(ListStart::At(T0))),
    );
    let key = |fx: &Fixture| {
        fx.cache
            .message(&fx.chan, &MessageId("m1".to_owned()))
            .served()
            .expect("served")
            .expect("held")
            .message
            .op_key
    };
    assert_eq!(key(&fx).as_deref(), Some("out-1.00000000deadbeef.0"));
    fx.list(
        vec![message("m1", "vibe-talk", "on it", T0)],
        T0 + 2 * MINUTE,
        Some(newest_proof(ListStart::At(T0))),
    );
    assert_eq!(key(&fx).as_deref(), Some("out-1.00000000deadbeef.0"));
}
