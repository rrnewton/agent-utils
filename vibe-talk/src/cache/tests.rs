//! Behaviour tests for the cache as a whole: the file, the writer, the merge rules applied, the
//! coverage stamps, the erase protocol and the bounds. `#208 persistent-cache`, milestone
//! `cache-store`.

use std::collections::BTreeSet;
use std::sync::atomic::AtomicI64;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use super::coverage::{Interval, Source, SETTLE_AFTER_LOCAL_MS, SETTLE_MS};
use super::fixtures::{self, message, newest_proof, reply, Fixture, ManualClock, T0};
use super::*;
use crate::model::{Message, Reaction};
use crate::threads::MessageThread;

pub(super) const STEP: i64 = 600_000;

/// Message `i`: created `i` steps of ten minutes after the origin.
pub(super) fn m(i: i64) -> Message {
    message(
        &format!("m{i}"),
        "ada",
        &format!("body number {i}"),
        T0 + i * STEP,
    )
}

pub(super) fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

pub(super) fn mid(id: &str) -> MessageId {
    MessageId(id.to_owned())
}

pub(super) fn push(fx: &Fixture, message: Message, update: bool, observed_ms: i64) {
    fx.offer(
        &fx.chan,
        Observation::Push {
            push: Push::Upsert {
                message: Box::new(Incoming::new(message, Scope::Main)),
                update,
                partial: false,
            },
            observed_ms,
        },
    );
}

pub(super) fn push_delete(fx: &Fixture, id: &str, observed_ms: i64) {
    fx.offer(
        &fx.chan,
        Observation::Push {
            push: Push::Delete {
                id: mid(id),
                created_ms: None,
            },
            observed_ms,
        },
    );
}

pub(super) fn held(fx: &Fixture, id: &str) -> Option<CachedMessage> {
    fx.cache
        .message(&fx.chan, &mid(id))
        .served()
        .expect("served")
}

pub(super) fn content(fx: &Fixture, id: &str) -> Option<String> {
    held(fx, id).map(|m| m.message.content)
}

pub(super) fn intervals(fx: &Fixture, scope: &Scope) -> Vec<Interval> {
    fx.cache.coverage(&fx.chan, scope).served().expect("served")
}

pub(super) fn sql_count(cache: &MessageCache, sql: &str) -> i64 {
    cache
        .read(None, |c| c.query_row(sql, [], |r| r.get(0)))
        .served()
        .expect("served")
}

/// The FTS integrity check against the content table, on a private connection: the readers are
/// query-only.
pub(super) fn integrity(fx: &Fixture) {
    let connection = rusqlite::Connection::open(fx.path()).expect("open");
    connection
        .execute(
            "INSERT INTO message_fts (message_fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .expect("the full-text index agrees with the message table");
}

// ---- the file ----------------------------------------------------------------------------------

#[test]
fn every_connection_has_its_pragmas_after_a_close_and_a_reopen() {
    let dir = crate::testing::TempDir::new("cache-pragmas");
    let clock = Arc::new(ManualClock(AtomicI64::new(T0)));
    let expected: Vec<(String, String)> = [
        ("secure_delete", "1"),
        ("synchronous", "1"),
        ("journal_size_limit", "4194304"),
        ("busy_timeout", "2000"),
        ("temp_store", "2"),
        ("foreign_keys", "1"),
        ("journal_mode", "wal"),
        ("application_id", "1987333443"),
        ("auto_vacuum", "2"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
    .collect();
    for _ in 0..2 {
        let cache = fixtures::open(&dir, &clock, Limits::default());
        let probed = cache.probe_pragmas();
        assert_eq!(
            probed.len(),
            4,
            "the writer, the checkpointer and both readers"
        );
        for (connection, pragmas) in probed.iter().enumerate() {
            assert_eq!(pragmas, &expected, "connection {connection}");
        }
        drop(cache);
    }
}

#[cfg(unix)]
#[test]
fn the_file_and_its_wal_and_shm_are_0600() {
    use std::os::unix::fs::PermissionsExt as _;
    let fx = Fixture::new("cache-modes");
    fx.list(vec![m(1)], T0, None);
    for member in file::family(&fx.path()).iter().take(3) {
        let mode = std::fs::metadata(member)
            .unwrap_or_else(|e| panic!("{} exists: {e}", member.display()))
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{}", member.display());
    }
    let dir_mode = std::fs::metadata(fx.path().parent().expect("dir"))
        .expect("dir")
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700);
}

#[test]
fn a_foreign_file_is_refused_and_left_exactly_as_it_was() {
    let dir = crate::testing::TempDir::new("cache-foreign");
    let clock = Arc::new(ManualClock(AtomicI64::new(T0)));
    let cache_dir = dir.path().join("cache");
    file::ensure_private_dir(&cache_dir).expect("dir");
    let path = cache_dir.join("c.sqlite3");
    let open = || {
        MessageCache::open(CacheOptions {
            clock: Arc::clone(&clock) as Arc<dyn Clock>,
            ..CacheOptions::new(&path)
        })
    };
    std::fs::write(&path, b"somebody else's notes").expect("write");
    assert!(matches!(open(), Err(CacheError::Foreign(_))));
    assert_eq!(
        std::fs::read(&path).expect("read"),
        b"somebody else's notes"
    );

    std::fs::remove_file(&path).expect("rm");
    let other = rusqlite::Connection::open(&path).expect("other db");
    other
        .execute_batch("CREATE TABLE theirs (x); INSERT INTO theirs VALUES (1);")
        .expect("theirs");
    drop(other);
    let before = std::fs::read(&path).expect("read");
    assert!(matches!(open(), Err(CacheError::Foreign(_))));
    assert_eq!(std::fs::read(&path).expect("read"), before);
}

#[test]
fn a_half_created_file_is_taken_as_ours_and_initialised() {
    let dir = crate::testing::TempDir::new("cache-half");
    let clock = Arc::new(ManualClock(AtomicI64::new(T0)));
    let cache_dir = dir.path().join("cache");
    file::ensure_private_dir(&cache_dir).expect("dir");
    let path = cache_dir.join("c.sqlite3");
    // An empty file: created, and nothing more, before a crash.
    file::create_if_absent(&path).expect("create");
    let cache = fixtures::open(&dir, &clock, Limits::default());
    assert_eq!(cache.counts().served().expect("counts"), Counts::default());
    drop(cache);
    file::unlink_family(&path);
    // A SQLite file with no application id and no schema.
    let bare = rusqlite::Connection::open(&path).expect("bare");
    bare.execute_batch("CREATE TABLE t (x); DROP TABLE t;")
        .expect("touch");
    drop(bare);
    assert_eq!(
        schema::read_header(&path).expect("header"),
        schema::Header::Unmarked
    );
    let cache = fixtures::open(&dir, &clock, Limits::default());
    let writer = cache.probe_pragmas().remove(0);
    assert!(
        writer.contains(&("auto_vacuum".to_owned(), "2".to_owned())),
        "{writer:?}"
    );
    drop(cache);
    assert_eq!(
        schema::read_header(&path).expect("header"),
        schema::Header::Ours
    );
}

#[test]
fn a_second_open_of_one_path_refuses_and_the_store_file_is_refused() {
    let fx = Fixture::new("cache-twice");
    let second = MessageCache::open(CacheOptions::new(fx.path()));
    assert!(matches!(second, Err(CacheError::Locked(_))), "{second:?}");
    let store = fx.dir.path().join("cache").join("store.sqlite3");
    let refused = MessageCache::open(CacheOptions {
        store_path: Some(store.clone()),
        ..CacheOptions::new(&store)
    });
    assert!(matches!(refused, Err(CacheError::Filesystem(_))));
}

#[test]
fn an_unreadable_file_of_ours_is_rebuilt() {
    let dir = crate::testing::TempDir::new("cache-corrupt");
    let clock = Arc::new(ManualClock(AtomicI64::new(T0)));
    let cache_dir = dir.path().join("cache");
    file::ensure_private_dir(&cache_dir).expect("dir");
    let path = cache_dir.join("c.sqlite3");
    let mut bytes = vec![0x5a_u8; 8192];
    bytes[..16].copy_from_slice(b"SQLite format 3\0");
    bytes[68..72].copy_from_slice(&schema::APPLICATION_ID_BYTES);
    std::fs::write(&path, &bytes).expect("write");
    let cache = fixtures::open(&dir, &clock, Limits::default());
    assert_eq!(cache.counts().served().expect("usable"), Counts::default());
}

#[test]
fn a_newer_additive_schema_is_used_and_an_incompatible_one_is_rebuilt() {
    let fx = Fixture::new("cache-compat");
    fx.list(vec![m(1)], T0, None);
    let path = fx.path();
    let Fixture {
        cache, clock, dir, ..
    } = fx;
    drop(cache);
    let poke = |version: i64, compat: &str| {
        let c = rusqlite::Connection::open(&path).expect("open");
        c.pragma_update(None, "user_version", version)
            .expect("version");
        c.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'schema_compat'",
            [compat],
        )
        .expect("compat");
    };
    let known = i64::try_from(schema::STEPS.len()).expect("a few steps");
    poke(known + 1, &known.to_string());
    let cache = fixtures::open(&dir, &clock, Limits::default());
    assert_eq!(cache.counts().served().expect("counts").messages, 1, "kept");
    drop(cache);
    poke(known + 1, &(known + 1).to_string());
    let cache = fixtures::open(&dir, &clock, Limits::default());
    assert_eq!(
        cache.counts().served().expect("counts").messages,
        0,
        "rebuilt"
    );
}

#[test]
fn migrations_are_additive_and_keep_what_the_file_holds() {
    let fx = Fixture::new("cache-migrate");
    fx.list(vec![m(1), m(2)], T0, None);
    let path = fx.path();
    drop(fx.cache);
    let connection = rusqlite::Connection::open(&path).expect("open");
    let mut ladder = schema::STEPS.to_vec();
    ladder.push(schema::Step {
        sql: "ALTER TABLE message ADD COLUMN later_column INTEGER;
              CREATE INDEX message_by_later ON message (later_column);",
        compat: 1,
    });
    schema::migrate(&connection, &ladder).expect("additive step");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("version");
    assert_eq!(version, i64::try_from(ladder.len()).expect("a few steps"));
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM message WHERE later_column IS NULL",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(rows, 2);
    // Applying the ladder again does nothing.
    schema::migrate(&connection, &ladder).expect("idempotent");
}

#[test]
fn a_file_of_schema_one_takes_the_record_of_what_was_left_out_and_keeps_its_rows() {
    let dir = crate::testing::TempDir::new("cache-migrate-v2");
    let clock = Arc::new(ManualClock(AtomicI64::new(T0)));
    let cache_dir = dir.path().join("cache");
    file::ensure_private_dir(&cache_dir).expect("dir");
    let path = cache_dir.join("c.sqlite3");
    file::create_if_absent(&path).expect("create");
    {
        let connection = rusqlite::Connection::open(&path).expect("open");
        schema::prepare_writer(&connection, schema::Header::Empty, &schema::STEPS[..1])
            .expect("schema 1");
        connection
            .execute_batch(
                "INSERT INTO channel (chan, provider_key, channel_id) VALUES (1, 'p', 'C1');
                 INSERT INTO scope_head (chan, scope_key, head_ms, unsettled_from_ms,
                        head_listed_ms) VALUES (1, 'main', 5, 6, 7);",
            )
            .expect("rows");
    }
    let cache = fixtures::open(&dir, &clock, Limits::default());
    assert_eq!(cache.counts().served().expect("counts").channels, 1, "kept");
    let (dropped, checked, checked_from): (Option<i64>, Option<i64>, Option<i64>) = cache
        .read(None, |c| {
            c.query_row(
                "SELECT c.dropped_ms, h.checked_ms, h.checked_from_ms FROM channel c
                 JOIN scope_head h ON h.chan = c.chan",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
        .served()
        .expect("served");
    assert_eq!((dropped, checked, checked_from), (None, None, None));
}

/// A head written before cache schema 3 does not say where the read that set it began listing, and
/// a membership does not say which read vouched for it: the band of such a head is vouched for by
/// nothing until the next read of the newest end, which records both.
#[test]
fn a_head_of_schema_two_vouches_for_no_band_until_the_next_read_of_the_newest_end() {
    let fx = Fixture::new("cache-migrate-v3");
    let proof = Some(newest_proof(ListStart::At(T0 + STEP)));
    let first = T0 + 6 * STEP + 1_000;
    fx.clock.set(first);
    fx.list((1..=6).map(m).collect(), first, proof);
    let path = fx.path();
    let Fixture {
        cache, clock, dir, ..
    } = fx;
    drop(cache);
    {
        // What a file of schema 2 held: no reach, no vouch.
        let c = rusqlite::Connection::open(&path).expect("open");
        c.execute_batch(
            "UPDATE scope_head SET checked_from_ms = NULL;
             UPDATE message_scope SET vouched_ms = NULL;",
        )
        .expect("as schema 2 left them");
    }
    let fx = Fixture {
        cache: fixtures::open(&dir, &clock, Limits::default()),
        chan: ChanKey::new("p", "C1"),
        clock,
        dir,
    };
    fx.cache
        .retain_only(RouteSnapshot::new([fx.chan.clone()], ["p".to_owned()]));
    fx.settle();
    let window = shadow::Ask::Window {
        consumer: shadow::Consumer::Messages,
        limit: 1,
    };
    let decide = |asked_ms: i64| {
        let query = shadow::query(&window, observe::Shape::DiscordNative, false).expect("query");
        let view = fx
            .cache
            .shadow_view(&fx.chan, &query)
            .served()
            .expect("served");
        let facts = shadow::Facts {
            asked_ms,
            live_fed: false,
            fed_since_ms: None,
            dropped_ms: None,
            barrier: true,
            feed: shadow::Feed::AdditionsOnly,
        };
        (shadow::decide(&window, &view, &facts), view)
    };
    let (decision, view) = decide(first + 1_000);
    assert_eq!(
        (view.head_checked_from_ms, view.rows[0].vouched_ms),
        (None, None)
    );
    assert_eq!(decision, shadow::Decision::Miss(shadow::Miss::Gap));
    let second = first + 5_000;
    fx.clock.set(second);
    fx.list((1..=6).map(m).collect(), second, proof);
    let (decision, view) = decide(second + 1_000);
    assert_eq!(
        (view.head_checked_from_ms, view.rows[0].vouched_ms),
        (Some(T0 + STEP), Some(second))
    );
    assert_eq!(decision, shadow::Decision::Serve);
}

// ---- the full-text index and privacy -----------------------------------------------------------

/// The oracle test: after mixed inserts, edits, deletions by push and by absence, and a cascade
/// erase, `MATCH` finds exactly what `instr` finds, and the index passes its integrity check.
#[test]
fn match_agrees_with_instr_through_mixed_writes_and_erases() {
    let fx = Fixture::new("cache-oracle");
    let words = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
    ];
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = |n: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % n
    };
    let pick = |n: u64| usize::try_from(n).unwrap_or(0);
    let other = ChanKey::new("p", "C2");
    let mut observed = T0;
    for round in 0..40_i64 {
        observed += 1_000;
        let mut rows = Vec::new();
        for _ in 0..(1 + next(6)) {
            let i = i64::try_from(next(30)).unwrap_or(0);
            let text = format!(
                "{} {} round{round}",
                words[pick(next(8))],
                words[pick(next(8))]
            );
            rows.push(message(&format!("m{i}"), "ada", &text, T0 + i * STEP));
        }
        let chan = if next(4) == 0 { &other } else { &fx.chan };
        let listing = |proof: Option<Proof>| {
            Observation::Listing(Listing {
                scope: Scope::Main,
                rows: rows
                    .iter()
                    .cloned()
                    .map(|r| Incoming::new(r, Scope::Main))
                    .collect(),
                observed_ms: observed,
                proof,
            })
        };
        match next(5) {
            0 => fx.offer(
                chan,
                Observation::Push {
                    push: Push::Delete {
                        id: mid(&format!("m{}", next(30))),
                        created_ms: None,
                    },
                    observed_ms: observed,
                },
            ),
            1 => fx.offer(chan, listing(Some(newest_proof(ListStart::ScopeStart)))),
            _ => fx.offer(chan, listing(None)),
        }
        if round == 30 {
            fx.cache.erase_channels(std::slice::from_ref(&other));
            fx.settle();
        }
        let terms = words
            .iter()
            .map(|w| (*w).to_owned())
            .chain([format!("round{round}"), "number".to_owned()]);
        for term in terms {
            let (by_index, by_scan) = fx
                .cache
                .read(None, |c| {
                    let quoted = search::fts_query(std::slice::from_ref(&term));
                    let mut a =
                        c.prepare("SELECT rowid FROM message_fts WHERE message_fts MATCH ?1")?;
                    let found: BTreeSet<i64> = a
                        .query_map([quoted], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    let mut b =
                        c.prepare("SELECT row_id FROM message WHERE instr(search_text, ?1) > 0")?;
                    let scanned: BTreeSet<i64> = b
                        .query_map([&term], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    Ok((found, scanned))
                })
                .served()
                .expect("served");
            assert_eq!(by_index, by_scan, "round {round}, term {term:?}");
        }
    }
    integrity(&fx);
    assert_eq!(fx.cache.stats().ops_failed, 0);
}

#[test]
fn a_tombstoned_unique_string_leaves_the_index_the_file_and_the_wal() {
    let fx = Fixture::new("cache-privacy");
    let secret = "zebracorn-quasar-71f3";
    fx.list(
        vec![
            message("keep", "ada", "ordinary words", T0),
            message(
                "gone",
                "ada",
                &format!("a message saying {secret} once"),
                T0 + 1,
            ),
        ],
        T0,
        None,
    );
    push_delete(&fx, "gone", T0 + 10);
    // The writer checkpointed after the tombstone batch; no `optimize` anywhere.
    let found: i64 = fx
        .cache
        .read(None, |c| {
            c.query_row(
                "SELECT COUNT(*) FROM message_fts WHERE message_fts MATCH ?1",
                [search::fts_query(&[secret.to_owned()])],
                |r| r.get(0),
            )
        })
        .served()
        .expect("served");
    assert_eq!(found, 0);
    assert!(fx.cache.stats().checkpoints_truncated >= 1);
    let needle = secret.as_bytes();
    for member in file::family(&fx.path()).iter().take(2) {
        let bytes = std::fs::read(member).unwrap_or_default();
        assert!(
            !bytes.windows(needle.len()).any(|w| w == needle),
            "{} still holds the deleted text",
            member.display()
        );
    }
    assert!(fx
        .cache
        .tombstoned(&fx.chan, &mid("gone"))
        .served()
        .expect("served"));
    integrity(&fx);
}

#[test]
fn listing_an_unchanged_row_again_writes_nothing() {
    let fx = Fixture::new("cache-unchanged");
    let rows = vec![m(1), m(2), m(3)];
    fx.list(rows.clone(), T0, None);
    let before = fx.cache.stats();
    assert_eq!(before.rows_written, 3);
    fx.list(rows, T0 + 5_000, None);
    let after = fx.cache.stats();
    assert_eq!(after.rows_written, 3, "nothing rewritten");
    assert_eq!(after.rows_unchanged, 3);
}

#[test]
fn every_batch_is_counted_in_the_duration_histogram_the_stats_carry() {
    let fx = Fixture::new("cache-batch-histogram");
    assert_eq!(fx.cache.stats().batch_us.count(), 0);
    assert_eq!(fx.cache.stats().batch_us.quantile_us(0.99), 0, "none yet");
    for i in 1..=5 {
        fx.list(vec![m(i)], T0 + i, None);
    }
    let stats = fx.cache.stats();
    assert!(stats.batches >= 1);
    assert_eq!(
        stats.batch_us.count(),
        stats.batches,
        "one duration per batch"
    );
    let (p50, p99) = (
        stats.batch_us.quantile_us(0.5),
        stats.batch_us.quantile_us(0.99),
    );
    assert!(
        p50 <= p99 && p99 <= stats.batch_max_us,
        "{p50} {p99} {}",
        stats.batch_max_us
    );
    let json = serde_json::to_value(&stats).expect("numbers serialize");
    assert_eq!(json["batch_us"]["count"], stats.batches);
    assert_eq!(json["batch_us"]["p99_us"], p99);
    assert!(json["batch_us"]["octaves_us"].is_array());
}

#[test]
fn a_shadow_view_reads_the_head_the_coverage_and_the_rows_and_honours_blocks() {
    let fx = Fixture::new("cache-shadow-view");
    let proof = newest_proof(ListStart::At(T0 + STEP));
    fx.list(vec![m(1), m(2), m(3), m(4)], T0 + 1_000, Some(proof));
    let applied = fx.cache.applied_seq();
    let view = |query: &shadow::Query| fx.cache.shadow_view(&fx.chan, query);
    let ids_of = |rows: &[CachedMessage]| -> Vec<String> {
        rows.iter().map(|row| row.message.id.0.clone()).collect()
    };

    let newest = view(&shadow::Query::Newest {
        scope: Scope::Main,
        limit: 2,
    })
    .served()
    .expect("served");
    assert_eq!(
        newest.head,
        fx.cache
            .head(&fx.chan, &Scope::Main)
            .served()
            .expect("served")
    );
    assert!(newest.head.is_some());
    assert_eq!(newest.coverage, intervals(&fx, &Scope::Main));
    assert_eq!(ids_of(&newest.rows), ids(&["m3", "m4"]));

    let span = |from: i64, until: Option<i64>, limit: usize| {
        view(&shadow::Query::Span {
            scope: Scope::Main,
            from_ms: from,
            until_ms: until,
            limit,
        })
        .served()
        .expect("served")
    };
    let between = span(T0 + 2 * STEP, Some(T0 + 4 * STEP), 10);
    assert_eq!(
        ids_of(&between.rows),
        ids(&["m2", "m3"]),
        "until is exclusive"
    );
    assert_eq!(between.head, newest.head);
    assert_eq!(ids_of(&span(T0 + 2 * STEP, None, 1).rows), ids(&["m2"]));

    let one = view(&shadow::Query::Message { id: mid("m2") })
        .served()
        .expect("served");
    assert_eq!(ids_of(&one.rows), ids(&["m2"]));
    assert_eq!(one.home, Some(Scope::Main));
    assert_eq!(one.coverage, intervals(&fx, &Scope::Main));
    assert_eq!(
        view(&shadow::Query::Message { id: mid("absent") }),
        Read::Served(shadow::View::default())
    );

    // The scope blocked: neither it nor a message at home in it is read.
    fx.cache.block_scope(&fx.chan, &Scope::Main);
    assert_eq!(
        view(&shadow::Query::Newest {
            scope: Scope::Main,
            limit: 2
        }),
        Read::Blocked
    );
    assert_eq!(
        view(&shadow::Query::Message { id: mid("m2") }),
        Read::Blocked
    );
    fx.cache.unblock(&fx.chan, Some(&Scope::Main));
    assert_eq!(fx.cache.blocks(), 0);
    fx.cache.block_channel(&fx.chan);
    assert_eq!(fx.cache.blocks(), 1);
    assert_eq!(
        view(&shadow::Query::Message { id: mid("m2") }),
        Read::Blocked
    );
    fx.cache.unblock(&fx.chan, None);

    let covered: i64 = intervals(&fx, &Scope::Main)
        .iter()
        .map(|interval| interval.t1_ms - interval.t0_ms)
        .sum();
    assert!(covered > 0);
    assert_eq!(fx.cache.covered_ms(), Read::Served(covered));
    assert_eq!(fx.cache.applied_seq(), applied, "reading wrote nothing");
}

// ---- the merge rules ---------------------------------------------------------------------------

#[test]
fn a_read_that_started_before_a_pushed_edit_never_reverts_it() {
    let fx = Fixture::new("cache-revert");
    fx.list(vec![m(1)], T0 + 100, None);
    let edited = Message {
        content: "edited body".to_owned(),
        ..m(1)
    };
    push(&fx, edited, true, T0 + 200);
    assert_eq!(content(&fx, "m1").as_deref(), Some("edited body"));
    // A read that started at 150 (or a snapshot as of 150) brings the old text: kept out.
    fx.list(vec![m(1)], T0 + 150, None);
    assert_eq!(content(&fx, "m1").as_deref(), Some("edited body"));
    // A later read with newer text replaces it.
    let again = Message {
        content: "edited twice".to_owned(),
        ..m(1)
    };
    fx.list(vec![again], T0 + 300, None);
    assert_eq!(content(&fx, "m1").as_deref(), Some("edited twice"));
}

#[test]
fn a_deletion_leaves_only_ids_and_times_and_only_a_later_read_brings_it_back() {
    let fx = Fixture::new("cache-tombstone");
    fx.list(vec![m(1)], T0 + 100, None);
    push_delete(&fx, "m1", T0 + 200);
    assert_eq!(content(&fx, "m1"), None);
    let columns: Vec<String> = fx
        .cache
        .read(None, |c| {
            let mut s = c.prepare("SELECT name FROM pragma_table_info('tombstone')")?;
            let names = s.query_map([], |r| r.get(0))?.collect();
            names
        })
        .served()
        .expect("served");
    assert_eq!(
        columns,
        ids(&["chan", "message_id", "created_ms", "deleted_ms"])
    );
    fx.list(vec![m(1)], T0 + 150, None);
    assert_eq!(content(&fx, "m1"), None, "a read from before the deletion");
    assert_eq!(fx.cache.stats().rows_held_by_tombstone, 1);
    fx.list(vec![m(1)], T0 + 250, None);
    assert!(content(&fx, "m1").is_some(), "a read from after it");
    assert!(!fx
        .cache
        .tombstoned(&fx.chan, &mid("m1"))
        .served()
        .expect("served"));
    // A deletion of an id the cache never held still leaves a tombstone.
    push_delete(&fx, "never-seen", T0 + 300);
    assert!(fx
        .cache
        .tombstoned(&fx.chan, &mid("never-seen"))
        .served()
        .expect("served"));
}

#[test]
fn absence_from_an_exhaustive_home_listing_spares_the_band_and_rows_pushed_meanwhile() {
    let fx = Fixture::new("cache-absence");
    let proof = newest_proof(ListStart::ScopeStart);
    fx.list(vec![m(1), m(2), m(3), m(10)], T0 + 100, Some(proof));
    // Pushed while the next read runs: observed at 150, after that read started at 120.
    push(&fx, m(4), false, T0 + 150);
    // In m10's band: a row created a minute before m10, seen before the read.
    let late = message("late", "ada", "late", T0 + 10 * STEP - 60_000);
    push(&fx, late, false, T0 + 110);
    // The read that started at 120 lacks m2, m4 and the late one.
    fx.list(vec![m(1), m(3), m(10)], T0 + 120, Some(proof));
    assert_eq!(content(&fx, "m2"), None, "absent and seen before: deleted");
    assert!(
        content(&fx, "m4").is_some(),
        "pushed after the read started"
    );
    assert!(content(&fx, "late").is_some(), "inside the unsettled band");
    assert!(fx
        .cache
        .tombstoned(&fx.chan, &mid("m2"))
        .served()
        .expect("served"));
    // A listing without a proof deletes nothing.
    fx.list(vec![m(10)], T0 + 130, None);
    assert!(content(&fx, "m1").is_some());
}

#[test]
fn a_message_never_held_is_absent_on_arrival_when_a_later_listing_proved_its_stretch_without_it() {
    let fx = Fixture::new("cache-absent-on-arrival");
    let rows = |rows: Vec<Message>, observed_ms: i64| Observation::Rows {
        rows: rows
            .into_iter()
            .map(|m| Incoming::new(m, Scope::Main))
            .collect(),
        scope: None,
        observed_ms,
    };
    // A read that started at 200 proves main from its start without m2 and m4.
    let proof = newest_proof(ListStart::ScopeStart);
    fx.list(vec![m(1), m(3), m(10)], T0 + 200, Some(proof));
    // A read that started at 150, before it, lands only now: m2 was gone by 200.
    fx.offer(&fx.chan, rows(vec![m(2)], T0 + 150));
    assert_eq!(content(&fx, "m2"), None, "a deleted message put back");
    assert!(fx
        .cache
        .tombstoned(&fx.chan, &mid("m2"))
        .served()
        .expect("served"));
    // So was m4, which the live feed saw at 190 and the poller publishes late.
    push(&fx, m(4), false, T0 + 190);
    assert_eq!(content(&fx, "m4"), None);
    assert_eq!(fx.cache.stats().rows_held_by_tombstone, 2);
    // A read that began after the listing says it exists again.
    fx.offer(&fx.chan, rows(vec![m(4)], T0 + 250));
    assert!(content(&fx, "m4").is_some());
    // The unsettled band proves nothing: a row seen before the listing, inside it, is held.
    let late = message("late", "ada", "late", T0 + 10 * STEP - 60_000);
    push(&fx, late, false, T0 + 150);
    assert!(content(&fx, "late").is_some());
    // Nor does a listing of main say anything of a reply, whose home is its thread.
    fx.offer(
        &fx.chan,
        Observation::Rows {
            rows: vec![Incoming::new(
                reply("r1", "m1", "a reply", T0 + 2 * STEP),
                Scope::Thread("m1".to_owned()),
            )],
            scope: None,
            observed_ms: T0 + 150,
        },
    );
    assert!(content(&fx, "r1").is_some());
}

#[test]
fn a_push_of_the_text_already_held_protects_the_row_from_an_older_listings_absence() {
    let fx = Fixture::new("cache-push-refresh");
    let proof = newest_proof(ListStart::At(T0));
    fx.list(vec![m(1), m(2), m(10)], T0 + 100, Some(proof));
    let before = fx.cache.stats().rows_written;
    // The live feed says m2 is there, as it is, at 200.
    push(&fx, m(2), true, T0 + 200);
    // A read that started at 150 lacks it: it began before the push saw it, so proves nothing.
    fx.list(vec![m(1), m(10)], T0 + 150, Some(proof));
    assert!(content(&fx, "m2").is_some());
    assert_eq!(
        held(&fx, "m2").expect("held").observed_ms,
        T0 + 200,
        "the push moved the time its text was last seen"
    );
    // A re-listing of unchanged rows still writes nothing.
    fx.list(vec![m(1), m(2), m(10)], T0 + 300, None);
    assert_eq!(
        fx.cache.stats().rows_written,
        before + 1,
        "only the push wrote"
    );
}

#[test]
fn listing_a_threads_replies_again_rewrites_neither_its_rows_nor_its_record() {
    let fx = Fixture::new("cache-thread-quiet");
    let replies = || {
        vec![
            reply("r1", "m1", "one", T0 + STEP + 1),
            reply("r2", "m1", "two", T0 + STEP + 2),
        ]
    };
    let thread = Scope::Thread("m1".to_owned());
    fx.list_scope(thread.clone(), replies(), T0 + 10, None);
    let verified = |fx: &Fixture| {
        sql_count(
            &fx.cache,
            "SELECT verified_ms FROM thread WHERE thread_id = 'm1'",
        )
    };
    assert_eq!(verified(&fx), T0 + 10);
    let written = fx.cache.stats().rows_written;
    fx.list_scope(thread, replies(), T0 + 50, None);
    assert_eq!(fx.cache.stats().rows_written, written);
    assert_eq!(verified(&fx), T0 + 10, "a reply carries no count to verify");
    // A thread list that counts the replies does verify the record.
    fx.offer(
        &fx.chan,
        Observation::Threads {
            threads: vec![ThreadRecord {
                thread_id: "m1".to_owned(),
                root_id: Some(mid("m1")),
                reply_count: Some(2),
                reply_count_exact: true,
                last_activity_ms: T0 + STEP + 2,
            }],
            observed_ms: T0 + 60,
        },
    );
    assert_eq!(verified(&fx), T0 + 60);
}

#[test]
fn absence_from_a_listing_of_another_scope_only_drops_that_membership() {
    let fx = Fixture::new("cache-membership");
    // A reply also sent to the channel: home is its thread, and it is listed in main too.
    let broadcast = reply("r1", "m1", "also in the channel", T0 + 2 * STEP);
    fx.offer(
        &fx.chan,
        Observation::Listing(Listing {
            scope: Scope::Main,
            rows: vec![
                Incoming::new(m(1), Scope::Main),
                Incoming::new(broadcast, Scope::Thread("m1".to_owned())),
                Incoming::new(m(5), Scope::Main),
            ],
            observed_ms: T0 + 100,
            proof: None,
        }),
    );
    assert_eq!(
        fx.cache
            .memberships(&fx.chan, &mid("r1"))
            .served()
            .expect("served"),
        vec![Scope::Main, Scope::Thread("m1".to_owned())]
    );
    fx.list(
        vec![m(1), m(5)],
        T0 + 200,
        Some(newest_proof(ListStart::ScopeStart)),
    );
    assert!(content(&fx, "r1").is_some(), "not deleted");
    assert_eq!(
        fx.cache
            .memberships(&fx.chan, &mid("r1"))
            .served()
            .expect("served"),
        vec![Scope::Thread("m1".to_owned())]
    );
}

#[test]
fn a_deleted_root_with_replies_leaves_its_replies_in_their_thread() {
    let fx = Fixture::new("cache-root");
    let mut root = m(1);
    root.thread = Some(MessageThread {
        id: "m1".to_owned(),
        root_message_id: Some(mid("m1")),
        is_root: true,
        reply_count: Some(2),
        reply_count_exact: true,
    });
    fx.list(vec![root, m(5)], T0 + 100, None);
    fx.list_scope(
        Scope::Thread("m1".to_owned()),
        vec![
            reply("r1", "m1", "one", T0 + STEP + 1),
            reply("r2", "m1", "two", T0 + STEP + 2),
        ],
        T0 + 100,
        None,
    );
    // The provider now answers the root as a tombstone, which the client filters out.
    fx.list(
        vec![m(5)],
        T0 + 200,
        Some(newest_proof(ListStart::ScopeStart)),
    );
    assert_eq!(content(&fx, "m1"), None);
    assert_eq!(fx.ids(&Scope::Thread("m1".to_owned())), ids(&["r1", "r2"]));
    assert!(fx
        .cache
        .thread(&fx.chan, "m1")
        .served()
        .expect("served")
        .is_some());
}

#[test]
fn a_complete_deltas_deletions_remove_ghosts_but_not_what_was_seen_after_it_started() {
    // A bridge supersedes a message's id; the ghost goes by the delta's deleted list.
    let fx = Fixture::new("cache-delta");
    fx.list_scope(Scope::Flat, vec![m(1), m(2)], T0 + 100, None);
    push(&fx, m(3), false, T0 + 300);
    fx.offer(
        &fx.chan,
        Observation::Deleted {
            ids: vec![mid("m1"), mid("m3"), mid("unknown")],
            observed_ms: T0 + 200,
        },
    );
    assert_eq!(content(&fx, "m1"), None, "the ghost is gone");
    assert!(content(&fx, "m2").is_some());
    assert!(
        content(&fx, "m3").is_some(),
        "set by a push after the delta's read started"
    );
    assert!(fx
        .cache
        .tombstoned(&fx.chan, &mid("unknown"))
        .served()
        .expect("served"));
    // ...and an exhaustive Flat listing that lacks a row removes it the same way.
    fx.list_scope(
        Scope::Flat,
        vec![m(3)],
        T0 + 400,
        Some(newest_proof(ListStart::ScopeStart)),
    );
    assert_eq!(content(&fx, "m2"), None);
    // A thread list's record is kept; its count only moves with a newer read.
    fx.offer(
        &fx.chan,
        Observation::Threads {
            threads: vec![ThreadRecord {
                thread_id: "t9".to_owned(),
                root_id: Some(mid("m3")),
                reply_count: Some(4),
                reply_count_exact: true,
                last_activity_ms: T0 + 9,
            }],
            observed_ms: T0 + 500,
        },
    );
    fx.offer(
        &fx.chan,
        Observation::Threads {
            threads: vec![ThreadRecord {
                thread_id: "t9".to_owned(),
                root_id: None,
                reply_count: Some(1),
                reply_count_exact: true,
                last_activity_ms: T0 + 5,
            }],
            observed_ms: T0 + 450,
        },
    );
    let record = fx
        .cache
        .thread(&fx.chan, "t9")
        .served()
        .expect("served")
        .expect("kept");
    assert_eq!(
        record.reply_count,
        Some(4),
        "an older read does not lower it"
    );
    assert_eq!(record.root_id, Some(mid("m3")));
    assert_eq!(record.last_activity_ms, T0 + 9);
}

#[test]
fn a_pushed_snippet_never_overwrites_listing_text_and_a_listing_replaces_a_partial() {
    let fx = Fixture::new("cache-snippet");
    let full = Message {
        content: "The whole body of a long message about the release".to_owned(),
        ..m(1)
    };
    fx.list(vec![full.clone()], T0 + 100, None);
    let snippet = Message {
        content: "The whole body of a long…".to_owned(),
        ..m(1)
    };
    push(&fx, snippet, true, T0 + 200);
    assert_eq!(content(&fx, "m1"), Some(full.content.clone()));
    // A pushed create of an unknown id is held as partial...
    let unknown = Message {
        content: "The start of".to_owned(),
        ..m(2)
    };
    push(&fx, unknown, false, T0 + 300);
    assert!(held(&fx, "m2").expect("held").content_partial);
    // ...and any listing replaces it, even one that started earlier.
    fx.list(vec![m(2)], T0 + 250, None);
    let replaced = held(&fx, "m2").expect("held");
    assert!(!replaced.content_partial);
    assert_eq!(replaced.message.content, m(2).content);
}

#[test]
fn a_copy_that_cannot_see_reactions_never_erases_held_ones() {
    let fx = Fixture::new("cache-reactions");
    let with = Message {
        reactions: Some(vec![Reaction {
            emoji: "👀".to_owned(),
            custom: false,
            custom_id: None,
            count: 2,
            owner_reacted: false,
        }]),
        ..m(1)
    };
    fx.list(vec![with.clone()], T0 + 100, None);
    push(&fx, m(1), true, T0 + 200);
    assert_eq!(
        held(&fx, "m1").expect("held").message.reactions,
        with.reactions
    );
    // Empty is an answer, and a newer read gives it.
    let none = Message {
        reactions: Some(Vec::new()),
        ..m(1)
    };
    fx.list(vec![none], T0 + 300, None);
    assert_eq!(
        held(&fx, "m1").expect("held").message.reactions,
        Some(Vec::new())
    );
}

#[test]
fn a_copy_that_named_no_mention_never_erases_names_a_read_found() {
    // `#318 slack-mention-names`. A live copy, or one from a path that resolves no names, keeps
    // the names a read found; a copy that names them anew replaces them.
    let fx = Fixture::new("cache-mention-names");
    let ada = |name: &str| {
        vec![crate::model::MentionName {
            id: "U0ADA0001".to_owned(),
            name: name.to_owned(),
        }]
    };
    let named = Message {
        content: "ask <@U0ADA0001>".to_owned(),
        mentions: ada("Ada Lovelace"),
        ..m(1)
    };
    fx.list(vec![named], T0, None);
    let unnamed = Message {
        content: "ask <@U0ADA0001>".to_owned(),
        ..m(1)
    };
    fx.list(vec![unnamed], T0 + 100, None);
    assert_eq!(
        held(&fx, "m1").expect("held").message.mentions,
        ada("Ada Lovelace")
    );
    let renamed = Message {
        content: "ask <@U0ADA0001>".to_owned(),
        mentions: ada("Ada King"),
        ..m(1)
    };
    fx.list(vec![renamed], T0 + 200, None);
    assert_eq!(
        held(&fx, "m1").expect("held").message.mentions,
        ada("Ada King")
    );
}

#[test]
fn an_edit_time_is_kept_and_only_ever_moves_later() {
    // `#320 slack-edited-mark`. A copy that cannot see edits never takes the mark away, and one read
    // before the last edit never moves it back.
    let fx = Fixture::new("cache-edited");
    let edited = |at: &str, content: &str| Message {
        content: content.to_owned(),
        edited_timestamp: Some(at.to_owned()),
        ..m(1)
    };
    fx.list(vec![m(1)], T0 + 100, None);
    assert_eq!(
        held(&fx, "m1").expect("held").message.edited_timestamp,
        None
    );
    fx.list(
        vec![edited("2026-10-14T10:05:00.000000Z", "fixed")],
        T0 + 200,
        None,
    );
    let first = held(&fx, "m1").expect("held").message;
    assert_eq!(first.content, "fixed");
    assert_eq!(
        first.edited_timestamp.as_deref(),
        Some("2026-10-14T10:05:00.000000Z")
    );
    // A live copy with the same words and no edit time.
    push(
        &fx,
        Message {
            content: "fixed".to_owned(),
            ..m(1)
        },
        true,
        T0 + 300,
    );
    assert_eq!(
        held(&fx, "m1")
            .expect("held")
            .message
            .edited_timestamp
            .as_deref(),
        Some("2026-10-14T10:05:00.000000Z"),
        "a copy that cannot see edits took the mark away"
    );
    // A later edit moves it on; an earlier one, read late, does not move it back.
    fx.list(
        vec![edited("2026-10-14T10:09:30.000100Z", "fixed twice")],
        T0 + 400,
        None,
    );
    fx.list(
        vec![edited("2026-10-14T10:05:00.000000Z", "fixed twice")],
        T0 + 500,
        None,
    );
    assert_eq!(
        held(&fx, "m1")
            .expect("held")
            .message
            .edited_timestamp
            .as_deref(),
        Some("2026-10-14T10:09:30.000100Z")
    );
}

#[test]
fn a_message_reads_back_exactly_as_the_client_produced_it_minus_derived_fields() {
    let fx = Fixture::new("cache-roundtrip");
    let mut original = reply("r9", "m1", "hello **there**", T0 + 7);
    original.reply_to = Some(mid("m1"));
    original.author_is_bot = true;
    original.edited_timestamp = Some("2026-10-14T10:05:00.000000Z".to_owned());
    original.reactions = Some(vec![Reaction {
        emoji: "party".to_owned(),
        custom: true,
        custom_id: Some("E1".to_owned()),
        count: 1,
        owner_reacted: true,
    }]);
    // `#318 slack-mention-names`: who a mention names comes back with the message.
    original.content = "hello **there** <@U0ADA0001>".to_owned();
    original.mentions = vec![crate::model::MentionName {
        id: "U0ADA0001".to_owned(),
        name: "Ada Lovelace".to_owned(),
    }];
    let mut decorated = original.clone();
    decorated.spoken_time = "09:51".to_owned();
    decorated.spoken_content = "hello there".to_owned();
    decorated.content_html = "<p>hello</p>".to_owned();
    decorated.noise = true;
    decorated.heading = Some(crate::model::Heading::default());
    fx.list_scope(Scope::Thread("m1".to_owned()), vec![decorated], T0, None);
    assert_eq!(held(&fx, "r9").expect("held").message, original);
}

#[test]
fn thread_records_need_replies_and_their_activity_only_moves_forward() {
    let fx = Fixture::new("cache-threads");
    let mut unanswered = m(1);
    unanswered.thread = Some(MessageThread {
        id: "m1".to_owned(),
        root_message_id: Some(mid("m1")),
        is_root: true,
        reply_count: Some(0),
        reply_count_exact: true,
    });
    fx.list(vec![unanswered], T0, None);
    assert_eq!(
        fx.cache.thread(&fx.chan, "m1").served().expect("served"),
        None
    );
    assert_eq!(
        sql_count(
            &fx.cache,
            "SELECT COUNT(*) FROM message WHERE thread_id IS NOT NULL"
        ),
        0,
        "an unanswered top-level message is not a thread"
    );
    fx.list_scope(
        Scope::Thread("m1".to_owned()),
        vec![reply("r2", "m1", "late reply", T0 + 5 * STEP)],
        T0 + 10,
        None,
    );
    fx.list_scope(
        Scope::Thread("m1".to_owned()),
        vec![reply("r1", "m1", "early reply", T0 + 2 * STEP)],
        T0 + 20,
        None,
    );
    let record = fx
        .cache
        .thread(&fx.chan, "m1")
        .served()
        .expect("served")
        .expect("a thread now");
    assert_eq!(record.last_activity_ms, T0 + 5 * STEP);
    fx.offer(
        &fx.chan,
        Observation::Threads {
            threads: vec![ThreadRecord {
                thread_id: "empty".to_owned(),
                root_id: None,
                reply_count: Some(0),
                reply_count_exact: true,
                last_activity_ms: T0,
            }],
            observed_ms: T0 + 30,
        },
    );
    assert_eq!(
        fx.cache.thread(&fx.chan, "empty").served().expect("served"),
        None
    );
}

#[test]
fn unrouted_channels_old_rows_and_coverage_below_the_age_limit_are_never_written() {
    let fx = Fixture::with_limits(
        "cache-never",
        Limits {
            max_days: 1,
            ..Limits::default()
        },
    );
    let stranger = ChanKey::new("p", "C9");
    fx.offer(
        &stranger,
        Observation::Listing(Listing {
            scope: Scope::Main,
            rows: vec![Incoming::new(m(1), Scope::Main)],
            observed_ms: T0,
            proof: None,
        }),
    );
    assert_eq!(fx.cache.stats().ops_unrouted, 1);
    assert_eq!(fx.cache.counts().served().expect("counts").messages, 0);
    // Local time is a week after the messages were written: older than the one-day limit.
    fx.clock.set(T0 + 7 * DAY_MS);
    fx.list(
        vec![m(1), m(2)],
        T0 + 7 * DAY_MS,
        Some(newest_proof(ListStart::ScopeStart)),
    );
    assert_eq!(fx.cache.counts().served().expect("counts").messages, 0);
    assert_eq!(fx.cache.stats().rows_too_old, 2);
    assert!(
        intervals(&fx, &Scope::Main).is_empty(),
        "coverage below the cutoff"
    );
}

// ---- coverage ----------------------------------------------------------------------------------

#[test]
fn only_a_proof_stamps_and_an_empty_listing_stamps_nothing() {
    let fx = Fixture::new("cache-proof");
    fx.list(vec![m(1), m(2), m(3)], T0 + 100, None);
    assert!(
        intervals(&fx, &Scope::Main).is_empty(),
        "a short read is not a proof"
    );
    // A proof from the oldest row, not from the scope's start: the start is never inferred.
    fx.list(
        vec![m(1), m(2), m(3)],
        T0 + 300,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let stamped = intervals(&fx, &Scope::Main);
    assert_eq!(stamped.len(), 1);
    assert_eq!(stamped[0].t0_ms, T0 + STEP);
    assert_eq!(
        stamped[0].t1_ms,
        T0 + 3 * STEP - SETTLE_MS,
        "below the band"
    );
    assert_eq!(stamped[0].listed_ms, T0 + 300);
    assert_eq!(stamped[0].full_ms, Some(T0 + 300));
    assert!(stamped[0].complete);
    // An empty page proves nothing, even with a proof, and even where nothing else would keep its
    // claim back: the head is far above it, so its span is settled.
    fx.list(
        vec![m(1), m(60)],
        T0 + 400,
        Some(newest_proof(ListStart::At(T0 + 60 * STEP))),
    );
    let before = intervals(&fx, &Scope::Main);
    let range = Proof {
        from: ListStart::At(T0 + 10 * STEP),
        until: ListEnd::Before(T0 + 20 * STEP),
        complete: true,
        full: true,
        source: Source::Range,
    };
    fx.list(Vec::new(), T0 + 500, Some(range));
    assert_eq!(
        intervals(&fx, &Scope::Main),
        before,
        "an empty page proves nothing"
    );
    // The same span with one row in it is a proof.
    fx.list(vec![m(15)], T0 + 600, Some(range));
    assert!(intervals(&fx, &Scope::Main)
        .iter()
        .any(|i| i.t0_ms == T0 + 10 * STEP && i.t1_ms == T0 + 20 * STEP));
    // A provider proof that the scope starts here stamps from its start (clamped to the age limit).
    fx.list(
        vec![m(1), m(2), m(3)],
        T0 + 700,
        Some(newest_proof(ListStart::ScopeStart)),
    );
    let cutoff = Limits::default().cutoff_ms("C1", T0);
    assert_eq!(intervals(&fx, &Scope::Main)[0].t0_ms, cutoff);
}

#[test]
fn the_oldest_coverage_is_where_the_earliest_proof_starts_and_goes_with_its_channel() {
    let fx = Fixture::new("cache-oldest-covered");
    assert_eq!(fx.cache.oldest_covered().served().expect("served"), None);
    fx.list(vec![m(1), m(2), m(3)], T0 + 300, None);
    assert_eq!(
        fx.cache.oldest_covered().served().expect("served"),
        None,
        "rows without a proof cover nothing"
    );
    fx.list(
        vec![m(4), m(5), m(6)],
        T0 + 400,
        Some(newest_proof(ListStart::At(T0 + 4 * STEP))),
    );
    fx.list(
        vec![m(2), m(3)],
        T0 + 500,
        Some(Proof {
            from: ListStart::At(T0 + 2 * STEP),
            until: ListEnd::Before(T0 + 4 * STEP),
            complete: true,
            full: true,
            source: Source::Range,
        }),
    );
    assert_eq!(
        fx.cache.oldest_covered().served().expect("served"),
        Some(T0 + 2 * STEP)
    );
    fx.cache.erase_channels(std::slice::from_ref(&fx.chan));
    fx.settle();
    assert_eq!(fx.cache.oldest_covered().served().expect("served"), None);
}

#[test]
fn coverage_stays_out_of_the_unsettled_band_until_a_later_read_settles_it() {
    let fx = Fixture::new("cache-band");
    let proof = newest_proof(ListStart::At(T0 + STEP));
    fx.list(vec![m(1), m(2), m(3)], T0 + 1_000, Some(proof));
    let head = fx
        .cache
        .head(&fx.chan, &Scope::Main)
        .served()
        .expect("served")
        .expect("head");
    assert_eq!(
        head.band(),
        Some((T0 + 3 * STEP - SETTLE_MS, T0 + 3 * STEP))
    );
    // A row inside the band, listed late, is normal: inserted, no contradiction.
    let late = || message("late", "ada", "late", T0 + 3 * STEP - 30_000);
    fx.list(vec![m(1), m(2), late(), m(3)], T0 + 2_000, Some(proof));
    assert!(content(&fx, "late").is_some());
    assert_eq!(
        intervals(&fx, &Scope::Main).last().map(|i| i.t1_ms),
        Some(T0 + 3 * STEP - SETTLE_MS)
    );
    // Ten minutes (local) after the head was listed, a read of the newest end settles the band.
    fx.list(
        vec![m(1), m(2), late(), m(3)],
        T0 + 1_000 + SETTLE_AFTER_LOCAL_MS,
        Some(proof),
    );
    let head = fx
        .cache
        .head(&fx.chan, &Scope::Main)
        .served()
        .expect("served")
        .expect("head");
    assert_eq!(head.band(), None);
    assert_eq!(
        intervals(&fx, &Scope::Main).last().map(|i| i.t1_ms),
        Some(T0 + 3 * STEP + 1)
    );
}

#[test]
fn a_bounded_flat_page_never_deletes_a_reply_or_stamps_a_thread() {
    // Slack's "All" view: Main plus the replies of at most ten threads, the newest fifty of each.
    let fx = Fixture::new("cache-flat");
    let roots: Vec<Message> = (0..11)
        .map(|i| {
            let mut root = m(i * 10);
            root.thread = Some(MessageThread {
                id: root.id.0.clone(),
                root_message_id: Some(root.id.clone()),
                is_root: true,
                reply_count: Some(if i == 0 { 60 } else { 1 }),
                reply_count_exact: true,
            });
            root
        })
        .collect();
    let replies = |count: i64| -> Vec<Incoming> {
        (0..count)
            .map(|n| {
                Incoming::new(
                    reply(&format!("r{n}"), "m0", "reply", T0 + 1_000 + n),
                    Scope::Thread("m0".to_owned()),
                )
            })
            .collect()
    };
    let flat_page = |observed_ms: i64, roots: &[Message], replies: Vec<Incoming>| {
        // Roots are a Main listing with the main part's proof; replies are upserted only.
        fx.offer(
            &fx.chan,
            Observation::Listing(Listing {
                scope: Scope::Main,
                rows: roots
                    .iter()
                    .cloned()
                    .map(|r| Incoming::new(r, Scope::Main))
                    .collect(),
                observed_ms,
                proof: Some(newest_proof(ListStart::At(T0))),
            }),
        );
        fx.offer(
            &fx.chan,
            Observation::Rows {
                rows: replies,
                scope: None,
                observed_ms,
            },
        );
    };
    flat_page(T0 + 100, &roots, replies(60));
    assert_eq!(fx.ids(&Scope::Thread("m0".to_owned())).len(), 60);
    // The next page is bounded: ten threads, and fifty of the sixty replies.
    flat_page(T0 + 200, &roots[..10], replies(50));
    assert_eq!(
        fx.ids(&Scope::Thread("m0".to_owned())).len(),
        60,
        "no reply was deleted by a bounded page"
    );
    assert_eq!(
        sql_count(
            &fx.cache,
            "SELECT COUNT(*) FROM coverage WHERE scope_key <> 'main'"
        ),
        0
    );
    assert_eq!(
        sql_count(
            &fx.cache,
            "SELECT COUNT(*) FROM scope_head WHERE scope_key <> 'main'"
        ),
        0
    );
    assert_eq!(
        sql_count(
            &fx.cache,
            "SELECT COUNT(*) FROM tombstone WHERE message_id LIKE 'r%'"
        ),
        0
    );
}

// ---- the writer --------------------------------------------------------------------------------

#[test]
fn an_offer_too_big_for_the_queue_is_dropped_and_marks_the_channel_suspect() {
    let fx = Fixture::new("cache-drop");
    fx.list(vec![m(1)], T0 - 20, Some(newest_proof(ListStart::At(T0))));
    let huge = Message {
        content: "x".repeat(QUEUE_BYTES),
        ..m(1)
    };
    let stamp = fx.cache.stamp(&fx.chan);
    let offered = fx.cache.offer(
        fx.chan.clone(),
        stamp,
        Observation::Listing(Listing {
            scope: Scope::Main,
            rows: vec![Incoming::new(huge, Scope::Main)],
            observed_ms: T0,
            proof: None,
        }),
    );
    assert_eq!(offered, Offered::Dropped);
    assert_eq!(fx.cache.last_drop_ms(&fx.chan), Some(T0));
    assert!(fx.cache.is_suspect(&fx.chan), "m1 was seen before the drop");
    assert_eq!(fx.cache.stats().ops_dropped, 1);
    // A read of the newest end that started after the drop sees m1 and the head again.
    fx.list(vec![m(1)], T0 + 20, Some(newest_proof(ListStart::At(T0))));
    assert!(!fx.cache.is_suspect(&fx.chan));
}

/// Another connection holds the cache file's write lock until the returned sender is dropped or
/// sent to, as a full disk, an I/O error or an operator's own session would make every write fail.
pub(super) fn hold_write_lock(
    cache: &MessageCache,
) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let path = cache.path().to_owned();
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let locker = std::thread::spawn(move || {
        let connection = rusqlite::Connection::open(&path).expect("open");
        connection.execute_batch("BEGIN IMMEDIATE").expect("lock");
        locked_tx.send(()).expect("tell");
        let _ = release_rx.recv();
        connection.execute_batch("ROLLBACK").expect("unlock");
    });
    locked_rx.recv().expect("locked");
    (release_tx, locker)
}

/// Wait until `done` says so, for at most thirty seconds.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(30), "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Writes that fail, past the writer's busy timeout, leave out what they carried as a full queue's
/// drop does. A drop whose own record cannot be written stays in memory and reaches the file once
/// it takes writes again; an edit whose write fails is a drop, in memory at once, and reaches the
/// file the same way. Each next start remembers them.
#[test]
fn a_write_that_fails_is_a_drop_and_the_file_records_it_once_it_takes_writes() {
    let dir = crate::testing::TempDir::new("cache-failed-write");
    let clock = Arc::new(ManualClock(AtomicI64::new(T0 + 7 * STEP)));
    let edited = ChanKey::new("p", "C1");
    let marked = ChanKey::new("p", "C2");
    let open = || {
        let cache = fixtures::open(&dir, &clock, Limits::default());
        cache.retain_only(RouteSnapshot::new(
            [edited.clone(), marked.clone()],
            ["p".to_owned()],
        ));
        assert!(cache.quiesce_blocking(Duration::from_secs(30)));
        cache
    };
    {
        let cache = open();
        for chan in [&edited, &marked] {
            let stamp = cache.stamp(chan);
            cache.offer(
                chan.clone(),
                stamp,
                Observation::Listing(Listing {
                    scope: Scope::Main,
                    rows: (1..=6).map(|i| Incoming::new(m(i), Scope::Main)).collect(),
                    observed_ms: T0 + 7 * STEP,
                    proof: Some(newest_proof(ListStart::At(T0 + STEP))),
                }),
            );
        }
        assert!(cache.quiesce_blocking(Duration::from_secs(30)));
        assert!(!cache.is_suspect(&edited) && !cache.is_suspect(&marked));
        // A drop, whose record the file cannot take.
        let (release, locker) = hold_write_lock(&cache);
        clock.set(T0 + 7 * STEP + 10);
        let applied = cache.stats().ops_applied;
        cache.mark_suspect(&marked);
        wait_for("the drop record was tried", || {
            cache.stats().ops_applied > applied
        });
        release.send(()).expect("release");
        locker.join().expect("locker");
        assert!(cache.quiesce_blocking(Duration::from_secs(30)));
        assert!(cache.is_suspect(&marked));
    }
    {
        let cache = open();
        assert!(
            cache.is_suspect(&marked),
            "the next start forgot the drop whose record failed"
        );
        assert!(!cache.is_suspect(&edited));
        // An edit, whose write fails.
        let (release, locker) = hold_write_lock(&cache);
        clock.set(T0 + 7 * STEP + 20);
        let stamp = cache.stamp(&edited);
        cache.offer(
            edited.clone(),
            stamp,
            Observation::Push {
                push: Push::Upsert {
                    message: Box::new(Incoming::new(
                        Message {
                            content: "edited upstream".to_owned(),
                            ..m(2)
                        },
                        Scope::Main,
                    )),
                    update: true,
                    partial: false,
                },
                observed_ms: T0 + 7 * STEP + 20,
            },
        );
        wait_for("the write failed", || cache.stats().ops_failed > 0);
        assert_eq!(
            cache.last_drop_ms(&edited),
            Some(T0 + 7 * STEP + 20),
            "the failed write is a drop, in memory at once"
        );
        release.send(()).expect("release");
        locker.join().expect("locker");
        assert!(cache.quiesce_blocking(Duration::from_secs(30)));
        assert!(cache.is_suspect(&edited));
        let text = cache
            .message(&edited, &mid("m2"))
            .served()
            .expect("served")
            .expect("held")
            .message
            .content;
        assert_eq!(text, "body number 2", "the edit never reached the file");
    }
    let cache = open();
    assert!(
        cache.is_suspect(&edited),
        "the next start forgot the failed write"
    );
}

/// A drop whose record the file could not take is owed, and retried after a backoff of a second
/// or more. A process that stops before then (its cache handle dropped, as at a normal shutdown)
/// still writes it, in the writer's last turn, once the file takes writes: the next start doubts
/// the channel.
#[test]
fn a_drop_record_still_owed_is_written_as_the_writer_stops() {
    let dir = crate::testing::TempDir::new("cache-drop-at-stop");
    let clock = Arc::new(ManualClock(AtomicI64::new(T0 + 7 * STEP)));
    let marked = ChanKey::new("p", "C2");
    let open = || {
        let cache = fixtures::open(&dir, &clock, Limits::default());
        cache.retain_only(RouteSnapshot::new([marked.clone()], ["p".to_owned()]));
        assert!(cache.quiesce_blocking(Duration::from_secs(30)));
        cache
    };
    {
        let cache = open();
        let stamp = cache.stamp(&marked);
        cache.offer(
            marked.clone(),
            stamp,
            Observation::Listing(Listing {
                scope: Scope::Main,
                rows: (1..=3).map(|i| Incoming::new(m(i), Scope::Main)).collect(),
                observed_ms: T0 + 7 * STEP,
                proof: Some(newest_proof(ListStart::At(T0 + STEP))),
            }),
        );
        assert!(cache.quiesce_blocking(Duration::from_secs(30)));
        let (release, locker) = hold_write_lock(&cache);
        clock.set(T0 + 7 * STEP + 10);
        // The drop's own record fails, and so does the step that writes it again; only then is
        // the file let go, and the step's next turn is a backoff away.
        let steps = cache.stats().steps;
        cache.mark_suspect(&marked);
        wait_for("the owed drop record was tried again, and failed", || {
            cache.stats().steps > steps
        });
        release.send(()).expect("release");
        locker.join().expect("locker");
        // Stopped at once, well inside the backoff: no retry has run.
    }
    let cache = open();
    assert!(
        cache.is_suspect(&marked),
        "the drop owed when the writer stopped was never written"
    );
}

#[test]
fn a_barrier_with_nothing_to_wait_for_leaves_the_flush_flag_down() {
    let fx = Fixture::new("cache-flush-flag");
    fx.list(vec![m(1)], T0, None);
    assert!(fx.cache.barrier_blocking(Duration::from_secs(1)));
    assert!(
        !fx.cache
            .shared
            .seq
            .flush
            .load(std::sync::atomic::Ordering::SeqCst),
        "a raised flag would cut the next batch short for no one"
    );
}

#[tokio::test]
async fn the_async_barrier_waits_for_what_was_offered_before_it() {
    let fx = Fixture::new("cache-barrier");
    let stamp = fx.cache.stamp(&fx.chan);
    let mut last = 0;
    for i in 0..50 {
        if let Offered::Queued(seq) = fx.cache.offer(
            fx.chan.clone(),
            stamp,
            Observation::Rows {
                rows: vec![Incoming::new(m(i), Scope::Main)],
                scope: None,
                observed_ms: T0,
            },
        ) {
            last = seq;
        }
    }
    assert!(fx.cache.barrier(Duration::from_secs(10)).await);
    assert!(fx.cache.applied_seq() >= last);
    assert_eq!(fx.cache.counts().served().expect("counts").messages, 50);
}

#[test]
fn search_text_is_rebuilt_when_the_rendering_rules_change() {
    struct Labelled;
    impl SearchTextSource for Labelled {
        fn version(&self) -> String {
            "labelled-2".to_owned()
        }
        fn rendered(&self, _provider_key: &str, message: &Message) -> Rendered {
            Rendered {
                text: format!("{} widgets#123", message.content),
                hrefs: vec!["https://example.test/issue/123".to_owned()],
            }
        }
    }
    let fx = Fixture::new("cache-rebuild");
    fx.list((0..1_200).map(m).collect(), T0, None);
    let path = fx.path();
    let Fixture {
        cache, clock, dir, ..
    } = fx;
    drop(cache);
    let cache = MessageCache::open(CacheOptions {
        search_text: Arc::new(Labelled),
        clock: Arc::clone(&clock) as Arc<dyn Clock>,
        ..CacheOptions::new(&path)
    })
    .expect("reopen");
    cache.maintain(Maintenance::RebuildSearchText);
    assert!(cache.quiesce_blocking(Duration::from_secs(30)));
    assert!(cache.search_text_current());
    let labelled = sql_count(
        &cache,
        "SELECT COUNT(*) FROM message_fts WHERE message_fts MATCH '\"widgets#123\"'",
    );
    assert_eq!(labelled, 1_200);
    drop(dir);
}

#[test]
fn a_rebuild_that_fails_is_retried_rather_than_taken_as_done() {
    struct Relabelled;
    impl SearchTextSource for Relabelled {
        fn version(&self) -> String {
            "relabelled-3".to_owned()
        }
        fn rendered(&self, _provider_key: &str, message: &Message) -> Rendered {
            Rendered {
                text: format!("{} relabelled", message.content),
                hrefs: Vec::new(),
            }
        }
    }
    let fx = Fixture::new("cache-rebuild-retry");
    fx.list((0..50).map(m).collect(), T0, None);
    let path = fx.path();
    let Fixture {
        cache, clock, dir, ..
    } = fx;
    drop(cache);
    let raw = rusqlite::Connection::open(&path).expect("raw");
    raw.execute_batch(
        "CREATE TRIGGER refuse_rebuild BEFORE UPDATE OF search_text ON message
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
    )
    .expect("sabotage");
    let cache = MessageCache::open(CacheOptions {
        search_text: Arc::new(Relabelled),
        clock: Arc::clone(&clock) as Arc<dyn Clock>,
        ..CacheOptions::new(&path)
    })
    .expect("reopen");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !cache.search_text_current(),
        "a rebuild that failed was reported done"
    );
    raw.execute_batch("DROP TRIGGER refuse_rebuild")
        .expect("repair");
    drop(raw);
    assert!(cache.quiesce_blocking(Duration::from_secs(30)));
    assert!(cache.search_text_current());
    assert_eq!(
        sql_count(
            &cache,
            "SELECT COUNT(*) FROM message_fts WHERE message_fts MATCH '\"relabelled\"'",
        ),
        50
    );
    drop(dir);
}

// ---- erase -------------------------------------------------------------------------------------

#[test]
fn an_observation_queued_before_an_erase_is_dropped_when_it_arrives() {
    let fx = Fixture::new("cache-erase-queued");
    fx.list(vec![m(1)], T0, None);
    let stamp = fx.cache.stamp(&fx.chan);
    // Queue an observation, then erase, and let both reach the writer in that order.
    fx.cache.offer(
        fx.chan.clone(),
        stamp,
        Observation::Rows {
            rows: vec![Incoming::new(m(2), Scope::Main)],
            scope: None,
            observed_ms: T0,
        },
    );
    fx.cache.erase_channels(std::slice::from_ref(&fx.chan));
    fx.settle();
    assert_eq!(fx.cache.counts().served().expect("counts").messages, 0);
    integrity(&fx);
}

#[test]
fn removing_a_channel_while_its_read_is_in_flight_leaves_nothing_afterwards() {
    let fx = Fixture::new("cache-erase-inflight");
    let other = ChanKey::new("p", "C2");
    // The read starts: its stamp is taken now.
    let stamp = fx.cache.stamp(&fx.chan);
    let other_stamp = fx.cache.stamp(&other);
    // The channel is removed while the read is running.
    fx.cache.erase_channels(std::slice::from_ref(&fx.chan));
    // The read lands.
    for (chan, stamp) in [(&fx.chan, stamp), (&other, other_stamp)] {
        fx.cache.offer(
            chan.clone(),
            stamp,
            Observation::Listing(Listing {
                scope: Scope::Main,
                rows: vec![Incoming::new(m(1), Scope::Main)],
                observed_ms: T0,
                proof: Some(newest_proof(ListStart::ScopeStart)),
            }),
        );
    }
    fx.settle();
    assert!(fx.ids(&Scope::Main).is_empty());
    assert!(intervals(&fx, &Scope::Main).is_empty());
    assert_eq!(fx.cache.stats().ops_stale, 1);
    assert_eq!(
        fx.cache
            .newest(&other, &Scope::Main, 10)
            .served()
            .expect("served")
            .len(),
        1,
        "another channel is untouched"
    );
}

#[test]
fn a_read_that_began_before_a_file_erase_never_returns_a_row_after_it() {
    let fx = Fixture::new("cache-erase-reader");
    fx.list(vec![m(1), m(2)], T0, None);
    let Fixture {
        cache, chan, dir, ..
    } = fx;
    let cache = Arc::new(cache);
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let reader = {
        let cache = Arc::clone(&cache);
        let chan = chan.clone();
        std::thread::spawn(move || {
            cache.read(Some(&chan), |c| {
                let _ = started_tx.send(());
                let _ = go_rx.recv_timeout(Duration::from_secs(10));
                c.query_row("SELECT COUNT(*) FROM message", [], |r| r.get::<_, i64>(0))
            })
        })
    };
    started_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the read began");
    let eraser = {
        let cache = Arc::clone(&cache);
        std::thread::spawn(move || cache.erase_file(true))
    };
    // Let the erase uninstall the file before the read runs its SQL.
    while cache.is_open() {
        std::thread::sleep(Duration::from_millis(1));
    }
    go_tx.send(()).expect("go");
    let answer = reader.join().expect("reader");
    assert_eq!(
        answer,
        Read::Moved,
        "rows read across an erase are never served"
    );
    let report = eraser.join().expect("eraser").expect("erased");
    assert!(report.reopened);
    assert_eq!(cache.counts().served().expect("counts").messages, 0);
    assert!(cache
        .newest(&chan, &Scope::Main, 10)
        .served()
        .expect("served")
        .is_empty());
    drop(dir);
}

#[test]
fn a_channel_being_erased_is_never_read_and_one_off_the_list_is_blocked() {
    let fx = Fixture::new("cache-erase-blocked");
    fx.list((0..200).map(m).collect(), T0, None);
    let other = ChanKey::new("p", "C2");
    fx.offer(
        &other,
        Observation::Rows {
            rows: vec![Incoming::new(m(1), Scope::Main)],
            scope: None,
            observed_ms: T0,
        },
    );
    // However the erase and this read interleave, the read never returns the erased rows.
    fx.cache.erase_channels(std::slice::from_ref(&fx.chan));
    match fx.cache.newest(&fx.chan, &Scope::Main, 1_000) {
        Read::Served(rows) => assert!(rows.is_empty(), "{} rows served", rows.len()),
        Read::Blocked | Read::Moved => {}
        other => panic!("{other:?}"),
    }
    fx.settle();
    assert!(fx.ids(&Scope::Main).is_empty());
    // A channel the route snapshot does not list is blocked, whatever the file holds.
    fx.cache
        .retain_only(RouteSnapshot::new([fx.chan.clone()], ["p".to_owned()]));
    assert_eq!(fx.cache.newest(&other, &Scope::Main, 10), Read::Blocked);
}

#[test]
fn a_file_erase_deletes_everything_and_stamps_from_before_it_are_stale() {
    let fx = Fixture::new("cache-erase-file");
    fx.list(vec![m(1)], T0, None);
    let old = fx.cache.stamp(&fx.chan);
    let report = fx.cache.erase_file(true).expect("erase");
    assert!(report.bytes > 0);
    // The route snapshot survives a file erase; the stamp from before it does not.
    fx.cache.offer(
        fx.chan.clone(),
        old,
        Observation::Rows {
            rows: vec![Incoming::new(m(2), Scope::Main)],
            scope: None,
            observed_ms: T0,
        },
    );
    fx.settle();
    assert_eq!(fx.cache.counts().served().expect("counts").messages, 0);
    assert_eq!(fx.cache.stats().ops_stale, 1);
    // Turning it off leaves no file at all, and nothing to read.
    let off = fx.cache.erase_file(false).expect("off");
    assert!(!off.reopened);
    assert!(!fx.path().exists());
    assert_eq!(fx.cache.counts(), Read::Off);
    let after = fx.cache.offer(
        fx.chan.clone(),
        fx.cache.stamp(&fx.chan),
        Observation::Deleted {
            ids: Vec::new(),
            observed_ms: T0,
        },
    );
    assert_eq!(after, Offered::Off);
    assert!(!fx.cache.barrier_blocking(Duration::from_millis(10)));
}

#[test]
fn a_crash_between_the_marker_and_the_unlink_erases_at_the_next_start() {
    let fx = Fixture::new("cache-crash");
    fx.list(vec![m(1)], T0, None);
    let marker_file = fx.dir.path().join("store.sqlite3.message-cache.json");
    let path = fx.path();
    // The owner's "off" writes the marker first...
    let mut sidecar = marker::Sidecar {
        off: true,
        off_at_ms: Some(T0),
        ..marker::Sidecar::default()
    };
    sidecar.register(&path);
    marker::store(&marker_file, &sidecar).expect("marker");
    // ...and the process dies before the unlink.
    let Fixture { cache, dir, .. } = fx;
    drop(cache);
    assert!(path.exists());
    let config = crate::config::MessageCacheConfig {
        enabled: true,
        ..crate::config::MessageCacheConfig::default()
    };
    let next_start = marker::reconcile_at_start(Some(&config), &marker_file, path.parent(), T0 + 1)
        .expect("reconcile");
    assert_eq!(next_start.action, marker::StartupAction::EraseAll);
    assert!(next_start.erased_bytes > 0);
    for member in file::family(&path) {
        assert!(!member.exists(), "{} survived", member.display());
    }
    drop(dir);
}

#[test]
fn switching_off_writes_the_marker_before_it_erases() {
    let fx = Fixture::new("cache-switch");
    fx.list(vec![m(1)], T0, None);
    let marker_file = fx.dir.path().join("store.sqlite3.message-cache.json");
    let report = marker::switch_off(&fx.cache, &marker_file, T0 + 5).expect("off");
    assert!(report.bytes > 0);
    let sidecar = marker::load(&marker_file).expect("load");
    assert!(sidecar.off);
    assert_eq!(sidecar.paths, vec![fx.path()]);
    assert!(!fx.path().exists());
}

#[test]
fn a_channel_erase_cascades_everywhere_and_the_index_stays_intact() {
    let fx = Fixture::new("cache-cascade");
    let other = ChanKey::new("p", "C2");
    let mut root = m(1);
    root.thread = Some(MessageThread {
        id: "m1".to_owned(),
        root_message_id: Some(mid("m1")),
        is_root: true,
        reply_count: Some(1),
        reply_count_exact: true,
    });
    fx.list(
        vec![root, m(2), m(3)],
        T0,
        Some(newest_proof(ListStart::ScopeStart)),
    );
    push_delete(&fx, "m2", T0 + 10);
    fx.offer(
        &other,
        Observation::Rows {
            rows: vec![Incoming::new(m(7), Scope::Main)],
            scope: None,
            observed_ms: T0,
        },
    );
    fx.cache.erase_channels(std::slice::from_ref(&fx.chan));
    fx.settle();
    for table in [
        "message",
        "tombstone",
        "coverage",
        "scope_head",
        "thread",
        "message_scope",
    ] {
        let left = sql_count(
            &fx.cache,
            &format!(
                "SELECT COUNT(*) FROM {table} WHERE chan NOT IN \
                 (SELECT chan FROM channel WHERE channel_id = 'C2')"
            ),
        );
        assert_eq!(left, 0, "{table} kept rows of the erased channel");
    }
    assert_eq!(fx.cache.counts().served().expect("counts").messages, 1);
    integrity(&fx);
    // Erasing a channel that holds nothing costs nothing.
    let erased = fx.cache.stats().erased_channels;
    fx.cache
        .erase_channels(&[ChanKey::new("p", "nothing-here")]);
    fx.settle();
    assert_eq!(fx.cache.stats().erased_channels, erased);
}

#[test]
fn retain_only_sweeps_every_channel_and_provider_off_the_list() {
    let fx = Fixture::new("cache-retain");
    let other = ChanKey::new("p", "C2");
    fx.list(vec![m(1)], T0, None);
    fx.offer(
        &other,
        Observation::Rows {
            rows: vec![Incoming::new(m(2), Scope::Main)],
            scope: None,
            observed_ms: T0,
        },
    );
    fx.cache
        .retain_only(RouteSnapshot::new([other.clone()], ["p".to_owned()]));
    fx.settle();
    let held_for = |channel_id: &str| {
        sql_count(
            &fx.cache,
            &format!(
                "SELECT COUNT(*) FROM message m JOIN channel c ON c.chan = m.chan \
                 WHERE c.channel_id = '{channel_id}'"
            ),
        )
    };
    assert_eq!(held_for("C1"), 0, "swept");
    assert_eq!(fx.cache.newest(&fx.chan, &Scope::Main, 10), Read::Blocked);
    assert_eq!(
        fx.cache
            .newest(&other, &Scope::Main, 10)
            .served()
            .expect("served")
            .len(),
        1
    );
    // ...and a later offer for the swept channel is refused at the gate.
    fx.list(vec![m(3)], T0 + 1, None);
    assert_eq!(held_for("C1"), 0);
    assert_eq!(fx.cache.stats().ops_unrouted, 1);
    fx.cache.erase_provider("p");
    fx.settle();
    assert_eq!(fx.cache.counts().served().expect("counts").messages, 0);
}

// ---- access in doubt ---------------------------------------------------------------------------

/// Channel `C1` holding main rows `m1`..`m3`, `m1` the root of thread `m1` with replies `r1` and
/// `r2` listed with a proof, and `C2` holding `m10`.
fn with_a_thread(tag: &str) -> (Fixture, ChanKey, Scope) {
    let fx = Fixture::new(tag);
    let other = ChanKey::new("p", "C2");
    let mut root = m(1);
    root.thread = Some(MessageThread {
        id: "m1".to_owned(),
        root_message_id: Some(mid("m1")),
        is_root: true,
        reply_count: Some(2),
        reply_count_exact: true,
    });
    fx.list(
        vec![root.clone(), m(2), m(3)],
        T0 + 3 * STEP,
        Some(newest_proof(ListStart::ScopeStart)),
    );
    let thread = Scope::Thread("m1".to_owned());
    fx.offer(
        &fx.chan,
        Observation::Listing(Listing {
            scope: thread.clone(),
            rows: vec![
                Incoming::new(root, Scope::Main),
                Incoming::new(reply("r1", "m1", "one", T0 + STEP + 1), thread.clone()),
                Incoming::new(reply("r2", "m1", "two", T0 + STEP + 2), thread.clone()),
            ],
            observed_ms: T0 + 3 * STEP,
            proof: Some(newest_proof(ListStart::ScopeStart)),
        }),
    );
    fx.offer(
        &other,
        Observation::Rows {
            rows: vec![Incoming::new(m(10), Scope::Main)],
            scope: None,
            observed_ms: T0,
        },
    );
    (fx, other, thread)
}

#[test]
fn a_blocked_channel_scope_or_provider_serves_nothing_until_unblocked_and_keeps_its_rows() {
    let (fx, other, thread) = with_a_thread("cache-blocks");
    let counts = fx.cache.counts().served().expect("counts");
    let served = |chan: &ChanKey, scope: &Scope| fx.cache.newest(chan, scope, 10) != Read::Blocked;

    fx.cache.block_channel(&fx.chan);
    assert_eq!(fx.cache.newest(&fx.chan, &Scope::Main, 10), Read::Blocked);
    assert_eq!(fx.cache.newest(&fx.chan, &thread, 10), Read::Blocked);
    assert_eq!(fx.cache.message(&fx.chan, &mid("m2")), Read::Blocked);
    assert_eq!(fx.cache.coverage(&fx.chan, &Scope::Main), Read::Blocked);
    assert!(served(&other, &Scope::Main), "another channel is served");
    // Observations are still applied: a read that succeeds is what lifts the doubt.
    fx.list(vec![m(4)], T0 + 4 * STEP, None);
    fx.cache.unblock(&fx.chan, None);
    assert_eq!(fx.ids(&Scope::Main), ids(&["m1", "m2", "m3", "m4"]));

    // One thread blocked: its scope, its record and its replies are not served; the channel's
    // main scope, and the root that lives there, are.
    fx.cache.block_scope(&fx.chan, &thread);
    assert_eq!(fx.cache.newest(&fx.chan, &thread, 10), Read::Blocked);
    assert_eq!(fx.cache.coverage(&fx.chan, &thread), Read::Blocked);
    assert_eq!(fx.cache.head(&fx.chan, &thread), Read::Blocked);
    assert_eq!(fx.cache.thread(&fx.chan, "m1"), Read::Blocked);
    assert_eq!(fx.cache.message(&fx.chan, &mid("r1")), Read::Blocked);
    assert_eq!(fx.cache.memberships(&fx.chan, &mid("m1")), Read::Blocked);
    assert!(served(&fx.chan, &Scope::Main));
    assert!(content(&fx, "m1").is_some(), "the root lives in main");
    fx.cache.unblock(&fx.chan, Some(&thread));
    assert_eq!(content(&fx, "r1").as_deref(), Some("one"));
    assert!(fx.cache.thread(&fx.chan, "m1").served().flatten().is_some());

    // A provider suspended: none of its channels is served.
    fx.cache.suspend_provider("p");
    assert_eq!(fx.cache.newest(&fx.chan, &Scope::Main, 10), Read::Blocked);
    assert_eq!(fx.cache.newest(&other, &Scope::Main, 10), Read::Blocked);
    fx.cache.resume_provider("p");
    assert!(served(&fx.chan, &Scope::Main) && served(&other, &Scope::Main));

    // Nothing was deleted by any of it.
    let after = fx.cache.counts().served().expect("counts");
    assert_eq!(after.messages, counts.messages + 1, "only m4 was added");
    assert_eq!(after.tombstones, counts.tombstones);
}

#[test]
fn erasing_a_thread_deletes_its_replies_as_deletions_and_leaves_the_rest_of_the_channel() {
    let (fx, other, thread) = with_a_thread("cache-erase-scope");
    let mut events = fx.cache.subscribe_events(64);
    fx.clock.set(T0 + 5 * STEP);
    fx.cache.erase_scope(&fx.chan, &thread);
    fx.settle();

    assert_eq!(content(&fx, "r1"), None);
    assert_eq!(content(&fx, "r2"), None);
    assert!(fx
        .cache
        .tombstoned(&fx.chan, &mid("r1"))
        .served()
        .expect("read"));
    assert_eq!(fx.ids(&thread), Vec::<String>::new());
    assert!(
        intervals(&fx, &thread).is_empty(),
        "no coverage claimed for it"
    );
    assert_eq!(fx.cache.head(&fx.chan, &thread), Read::Served(None));
    assert_eq!(fx.cache.thread(&fx.chan, "m1"), Read::Served(None));
    // The root lives in main, and main keeps everything it held, coverage included.
    assert_eq!(fx.ids(&Scope::Main), ids(&["m1", "m2", "m3"]));
    assert_eq!(
        fx.cache.memberships(&fx.chan, &mid("m1")),
        Read::Served(vec![Scope::Main])
    );
    assert_eq!(intervals(&fx, &Scope::Main).len(), 1);
    assert_eq!(
        fx.cache
            .newest(&other, &Scope::Main, 10)
            .served()
            .expect("served")
            .len(),
        1
    );
    // Each reply is told to the store as a deletion; nothing says the channel went.
    let mut told = drain(&mut events);
    told.sort_by_key(|event| format!("{event:?}"));
    assert_eq!(told, [orphan("C1", "r1", None), orphan("C1", "r2", None)]);

    // A read that started before the erase cannot bring a reply back; one after it can.
    fx.list_scope(
        thread.clone(),
        vec![reply("r1", "m1", "one", T0 + STEP + 1)],
        T0 + 4 * STEP,
        None,
    );
    assert_eq!(content(&fx, "r1"), None);
    fx.list_scope(
        thread,
        vec![reply("r1", "m1", "one", T0 + STEP + 1)],
        T0 + 6 * STEP,
        None,
    );
    assert_eq!(content(&fx, "r1").as_deref(), Some("one"));
}

#[test]
fn a_scope_erase_blocks_its_channel_until_applied_and_drops_fills_already_running() {
    let (fx, _other, thread) = with_a_thread("cache-erase-scope-race");
    let stamp = fx.cache.stamp(&fx.chan);
    let resume = fx.cache.pause_writer();
    fx.cache.erase_scope(&fx.chan, &thread);
    assert_eq!(fx.cache.newest(&fx.chan, &Scope::Main, 10), Read::Blocked);
    drop(resume);
    fx.settle();
    // A read of the thread that started before the erase lands after it: dropped.
    fx.cache.offer(
        fx.chan.clone(),
        stamp,
        Observation::Rows {
            rows: vec![Incoming::new(
                reply("r3", "m1", "three", T0 + STEP + 3),
                thread.clone(),
            )],
            scope: None,
            observed_ms: T0 + 6 * STEP,
        },
    );
    fx.settle();
    assert_eq!(content(&fx, "r3"), None);
    assert_eq!(fx.ids(&Scope::Main), ids(&["m1", "m2", "m3"]));
}

// ---- accounts ----------------------------------------------------------------------------------

#[test]
fn an_account_is_written_read_back_and_erased_with_its_provider() {
    let fx = Fixture::new("cache-account");
    assert_eq!(fx.cache.account("p"), Read::Served(None), "none yet");

    fx.clock.set(T0 + 5);
    assert!(fx.cache.set_account("p", "U1", Some("T1")).is_some());
    fx.settle();
    let written = fx.cache.account("p").served().flatten().expect("written");
    assert_eq!(
        written,
        Account {
            account_id: "U1".to_owned(),
            workspace_id: Some("T1".to_owned()),
            verified_ms: T0 + 5,
        }
    );
    assert!(written.is("U1", Some("T1")));
    assert!(!written.is("U1", None), "the workspace must match too");
    assert!(!written.is("U2", Some("T1")));

    // Written again, it is updated in place: one row per provider.
    fx.cache.set_account("p", "U2", None);
    fx.settle();
    assert!(fx
        .cache
        .account("p")
        .served()
        .flatten()
        .expect("held")
        .is("U2", None));
    assert_eq!(sql_count(&fx.cache, "SELECT COUNT(*) FROM account"), 1);

    // A provider the route snapshot does not configure gets no account row.
    let unrouted = fx.cache.stats().ops_unrouted;
    fx.cache.set_account("q", "U9", None);
    fx.settle();
    assert_eq!(fx.cache.account("q"), Read::Served(None));
    assert_eq!(fx.cache.stats().ops_unrouted, unrouted + 1);

    // While an erase of the provider waits for the writer, its account is not read: the row
    // found would be the one being erased.
    let resume = fx.cache.pause_writer();
    fx.cache.erase_provider("p");
    assert_eq!(fx.cache.account("p"), Read::Blocked);
    drop(resume);
    fx.settle();
    assert_eq!(fx.cache.account("p"), Read::Served(None), "erased with it");

    // An erase then a write, in that order, leave the new account: one queue, applied in order.
    fx.cache.set_account("p", "U1", Some("T1"));
    let resume = fx.cache.pause_writer();
    fx.cache.erase_provider("p");
    fx.cache.set_account("p", "U3", Some("T1"));
    drop(resume);
    fx.settle();
    assert!(fx
        .cache
        .account("p")
        .served()
        .flatten()
        .expect("held")
        .is("U3", Some("T1")));

    // A file erase takes it, and the generation moves on.
    let generation = fx.cache.generation();
    fx.cache.erase_file(true).expect("reopened");
    assert_ne!(fx.cache.generation(), generation);
    assert_eq!(fx.cache.account("p"), Read::Served(None));
}

// ---- bounds and maintenance --------------------------------------------------------------------

#[test]
fn eviction_converges_below_the_size_limit_in_a_bounded_number_of_chunks() {
    let fx = Fixture::with_limits(
        "cache-evict",
        Limits {
            max_bytes: 4 * 1024 * 1024 * 1024,
            ..Limits::default()
        },
    );
    let body = "lorem ipsum dolor sit amet ".repeat(40);
    let rows = 12_000_i64;
    for chunk in 0..(rows / 500) {
        fx.list(
            (chunk * 500..(chunk + 1) * 500)
                .map(|i| {
                    message(
                        &format!("m{i}"),
                        "ada",
                        &format!("{body} {i}"),
                        T0 + i * 1_000,
                    )
                })
                .collect(),
            T0,
            Some(Proof {
                from: ListStart::At(T0 + chunk * 500 * 1_000),
                until: ListEnd::Before(T0 + (chunk + 1) * 500 * 1_000),
                complete: true,
                full: true,
                source: Source::Walk,
            }),
        );
    }
    let path = fx.path();
    let Fixture {
        cache,
        clock,
        dir,
        chan,
    } = fx;
    drop(cache);
    let full = file::bytes_on_disk(&path);
    let limit: u64 = 16 * 1024 * 1024;
    assert!(full > limit * 2, "the fill must be over the limit: {full}");
    let cache = fixtures::open(
        &dir,
        &clock,
        Limits {
            max_bytes: limit,
            ..Limits::default()
        },
    );
    let mut events = cache.subscribe_events(64);
    cache.maintain(Maintenance::Bounds);
    assert!(cache.quiesce_blocking(Duration::from_secs(60)));
    let stats = cache.stats();
    assert!(
        events.try_recv().is_err(),
        "rows the cache evicts say nothing about their messages"
    );
    assert!(
        cache.bytes_on_disk() <= limit,
        "{} > {limit}",
        cache.bytes_on_disk()
    );
    // Bounded: every chunk deletes at least one row and at most a thousand, and the rounds that
    // measure, delete and compact stop at a handful, so the chunks are bounded by the rows.
    let rows = u64::try_from(rows).unwrap_or(0);
    assert!(stats.evicted_rows > 0);
    assert!(stats.evicted_rows < rows, "evicted everything: {stats:?}");
    assert!(
        stats.eviction_chunks <= stats.evicted_rows,
        "{} chunks for {} rows",
        stats.eviction_chunks,
        stats.evicted_rows
    );
    assert!(stats.eviction_chunks >= stats.evicted_rows / u64::try_from(DELETE_CHUNK).unwrap_or(1));
    assert!(stats.compactions >= 1, "eviction compacts the index");
    // Coverage never claims what eviction removed.
    let oldest: i64 = cache
        .read(None, |c| {
            c.query_row("SELECT MIN(created_ms) FROM message", [], |r| r.get(0))
        })
        .served()
        .expect("served");
    cache.retain_only(RouteSnapshot::new([chan.clone()], ["p".to_owned()]));
    assert!(cache.barrier_blocking(Duration::from_secs(10)));
    let stamped = cache
        .coverage(&chan, &Scope::Main)
        .served()
        .expect("served");
    assert!(!stamped.is_empty());
    assert!(
        stamped.iter().all(|i| i.t0_ms > oldest - 1_000),
        "{stamped:?} vs {oldest}"
    );
}

#[test]
fn the_hourly_pass_expires_old_rows_unrevalidated_rows_and_old_tombstones() {
    let fx = Fixture::with_limits(
        "cache-hourly",
        Limits {
            max_days: 30,
            channel_days: [("C2".to_owned(), 1)].into_iter().collect(),
            ..Limits::default()
        },
    );
    let other = ChanKey::new("p", "C2");
    // Revalidated: an exhaustive, edit-aware listing covers m1..m3.
    fx.list(
        vec![m(1), m(2), m(3)],
        T0,
        Some(newest_proof(ListStart::At(T0))),
    );
    // Never revalidated: a row only seen once, without a proof.
    fx.list_scope(Scope::Flat, vec![m(20)], T0, None);
    fx.offer(
        &other,
        Observation::Rows {
            rows: vec![Incoming::new(m(30), Scope::Main)],
            scope: None,
            observed_ms: T0,
        },
    );
    let mut events = fx.cache.subscribe_events(64);
    push_delete(&fx, "m3", T0 + 1);
    // Two days on: the one-day channel's row has aged out; nothing else has.
    fx.clock.set(T0 + 2 * DAY_MS);
    fx.cache.maintain(Maintenance::Hourly);
    fx.settle();
    assert!(fx
        .cache
        .newest(&other, &Scope::Main, 10)
        .served()
        .expect("served")
        .is_empty());
    assert!(content(&fx, "m20").is_some());
    assert_eq!(fx.cache.counts().served().expect("counts").tombstones, 1);
    // Eight days on: what nothing revalidated within a week is gone, and so is the tombstone.
    fx.clock.set(T0 + 8 * DAY_MS);
    fx.cache.maintain(Maintenance::Hourly);
    fx.settle();
    assert_eq!(content(&fx, "m20"), None, "never revalidated");
    assert_eq!(
        content(&fx, "m1"),
        None,
        "its covering read is more than a week old"
    );
    assert_eq!(fx.cache.counts().served().expect("counts").tombstones, 0);
    assert_eq!(fx.cache.stats().integrity_failures, 0);
    // The deletion was the message's own; the rows the age limit and the week took say nothing
    // about theirs.
    assert_eq!(
        drain(&mut events),
        [orphan("C1", "m3", None)],
        "only the deletion is told to the store"
    );
}

#[test]
fn revalidation_within_the_week_keeps_rows() {
    let fx = Fixture::new("cache-revalidate");
    fx.list(vec![m(1), m(2)], T0, Some(newest_proof(ListStart::At(T0))));
    fx.clock.set(T0 + 6 * DAY_MS);
    fx.list(
        vec![m(1), m(2)],
        T0 + 6 * DAY_MS,
        Some(newest_proof(ListStart::At(T0))),
    );
    fx.clock.set(T0 + 8 * DAY_MS);
    fx.cache.maintain(Maintenance::Hourly);
    fx.settle();
    assert_eq!(fx.ids(&Scope::Main), ids(&["m1", "m2"]));
}

/// The week takes the rows of a stretch nothing re-read within it, and the claim over that stretch
/// goes in the same pass: a claim that every message of the stretch is held, kept over rows it has
/// just deleted, would have a page served without them. The head goes too, with the rows of its
/// band.
#[test]
fn revalidation_never_leaves_a_claim_or_a_head_over_rows_it_deleted() {
    let fx = Fixture::new("cache-revalidate-claims");
    fx.list(
        vec![m(1), m(2), m(3), m(4), m(5), m(6)],
        T0 + 6 * STEP + 1_000,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    assert!(
        !intervals(&fx, &Scope::Main).is_empty(),
        "a claim was stamped"
    );
    let now = T0 + 8 * DAY_MS;
    fx.clock.set(now);
    fx.cache.maintain(Maintenance::Hourly);
    fx.settle();
    assert_eq!(content(&fx, "m2"), None, "the week took the row");
    assert_eq!(intervals(&fx, &Scope::Main), Vec::new(), "nor its claim");
    assert_eq!(
        fx.cache
            .head(&fx.chan, &Scope::Main)
            .served()
            .expect("served"),
        None,
        "nor the head whose band it held"
    );
    let ask = shadow::Ask::Page {
        since_ms: T0 + STEP,
        until_ms: Some(T0 + 4 * STEP),
        limit: 20,
    };
    let query = shadow::query(&ask, observe::Shape::DiscordNative, false).expect("a query");
    let view = fx
        .cache
        .shadow_view(&fx.chan, &query)
        .served()
        .expect("served");
    let facts = shadow::Facts {
        asked_ms: now + 1,
        live_fed: false,
        fed_since_ms: None,
        dropped_ms: None,
        barrier: true,
        feed: shadow::Feed::AdditionsOnly,
    };
    assert_eq!(
        shadow::decide(&ask, &view, &facts),
        shadow::Decision::Miss(shadow::Miss::Gap),
        "a page of {} rows served from a claim whose rows are gone",
        view.rows.len()
    );
}

/// A claim re-read within the week survives the pass with its rows, and one that was not goes,
/// though rows under it were seen since by a push: a claim is weakened, never invented.
#[test]
fn revalidation_keeps_a_claim_read_within_the_week_and_drops_one_that_was_not() {
    let fx = Fixture::new("cache-revalidate-some");
    fx.list(
        vec![m(1), m(2), m(3)],
        T0 + 3 * STEP + 1_000,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    // Six days on, the newer end is read again, and a push says m1 is there as it is.
    let later = T0 + 6 * DAY_MS;
    fx.clock.set(later);
    fx.list(
        vec![m(3), m(4), m(5), m(6)],
        later,
        Some(newest_proof(ListStart::At(T0 + 3 * STEP))),
    );
    push(&fx, m(1), true, later);
    fx.clock.set(T0 + 8 * DAY_MS);
    fx.cache.maintain(Maintenance::Hourly);
    fx.settle();
    assert_eq!(
        content(&fx, "m2"),
        None,
        "nothing re-read m2 within the week"
    );
    assert!(content(&fx, "m1").is_some(), "the push saw m1 within it");
    let claims = intervals(&fx, &Scope::Main);
    assert!(
        claims.iter().all(|claim| claim.t0_ms >= T0 + 3 * STEP),
        "only the stretch read within the week is still claimed: {claims:?}"
    );
    assert!(
        coverage::covers(&claims, T0 + 3 * STEP, T0 + 5 * STEP).is_some(),
        "{claims:?}"
    );
}

/// Twenty rows read at once; an hour later (a message edited upstream meanwhile, on a feed that
/// carries no edits) a read of the newest two. The head is fresh again, but a window of twenty
/// rests on the claim from an hour before, and is not served as fresh.
#[test]
fn a_short_newest_read_never_makes_an_older_claim_fresh() {
    let fx = Fixture::new("cache-short-fresh");
    let first = T0 + 20 * STEP + 1_000;
    fx.clock.set(first);
    fx.list(
        (1..=20).map(m).collect(),
        first,
        Some(newest_proof(ListStart::At(T0 + STEP))),
    );
    let later = first + 3_600_000;
    fx.clock.set(later);
    fx.list(
        vec![m(19), m(20)],
        later,
        Some(newest_proof(ListStart::At(T0 + 19 * STEP))),
    );
    let decide = |limit: usize| {
        let ask = shadow::Ask::Window {
            consumer: shadow::Consumer::Digest,
            limit,
        };
        let query = shadow::query(&ask, observe::Shape::DiscordNative, false).expect("a query");
        let view = fx
            .cache
            .shadow_view(&fx.chan, &query)
            .served()
            .expect("served");
        let facts = shadow::Facts {
            asked_ms: later + 5_000,
            live_fed: false,
            fed_since_ms: None,
            dropped_ms: None,
            barrier: true,
            feed: shadow::Feed::AdditionsOnly,
        };
        shadow::decide(&ask, &view, &facts)
    };
    assert_eq!(
        decide(20),
        shadow::Decision::Miss(shadow::Miss::StaleHead),
        "twenty rows served as fresh, the oldest last read an hour before"
    );
    assert_eq!(decide(2), shadow::Decision::Serve, "the two re-read are");
}

// ---- the unsettled band ------------------------------------------------------------------------

/// Every read of a scope's newest end that is the latest yet records its time and where its listing
/// began, and vouches for the rows of the band it listed: their memberships of the scope record its
/// start, and their times move up to it. A row of the band it did not list keeps its older vouch
/// and time; a read that began earlier and lands late moves neither the head nor either, and
/// vouches for nothing, whatever newer row it carries.
#[test]
fn only_the_latest_read_of_the_newest_end_vouches_for_the_band() {
    let fx = Fixture::new("cache-band-vouch");
    let proof = Some(newest_proof(ListStart::At(T0 + STEP)));
    // Written thirty seconds before m6: inside the band of any head at m6.
    let posted = message("b1", "ada", "posted and deleted", T0 + 6 * STEP - 30_000);
    let rows = || (1..=6).map(m).collect::<Vec<_>>();
    let first = T0 + 7 * STEP;
    let mut with_b1 = rows();
    with_b1.push(posted.clone());
    fx.list(with_b1, first, proof);
    // A minute later b1 is gone, and the latest read lists the channel without it.
    let second = first + 60_000;
    fx.list(rows(), second, proof);
    let head = fx
        .cache
        .head(&fx.chan, &Scope::Main)
        .served()
        .expect("served")
        .expect("a head");
    let observed = |id: &str| held(&fx, id).expect("held").observed_ms;
    assert!(
        held(&fx, "b1").is_some(),
        "inside the band an absence proves nothing yet"
    );
    assert_eq!(observed("b1"), first, "not vouched for by the latest read");
    assert_eq!(observed("m6"), second, "vouched for by the latest read");
    assert_eq!(
        observed("m3"),
        first,
        "below the band a claim holds it, and nothing moved"
    );
    // A read that began before the second one, carrying a newer message and b1, lands now.
    let mut late = rows();
    late.push(posted);
    late.push(m(7));
    fx.list(late, first + 30_000, proof);
    assert_eq!(
        fx.cache
            .head(&fx.chan, &Scope::Main)
            .served()
            .expect("served"),
        Some(head),
        "the late read moved the head"
    );
    assert_eq!(observed("b1"), first);
    assert_eq!(observed("m7"), first + 30_000);
    // What a newest page of five would be answered with holds m7 and b1: neither is vouched for.
    let ask = shadow::Ask::Timeline {
        view: crate::threads::TimelineView::Main,
        thread: None,
        limit: 5,
    };
    let query = shadow::query(&ask, observe::Shape::DiscordNative, false).expect("a query");
    let view = fx
        .cache
        .shadow_view(&fx.chan, &query)
        .served()
        .expect("served");
    assert_eq!(view.head_checked_ms, Some(second));
    assert_eq!(view.head_checked_from_ms, Some(T0 + STEP));
    let vouched = |id: &str| {
        view.rows
            .iter()
            .find(|row| row.message.id.as_str() == id)
            .expect("read")
            .vouched_ms
    };
    assert_eq!(
        (vouched("b1"), vouched("m6"), vouched("m7")),
        (Some(first), Some(second), None)
    );
    let facts = shadow::Facts {
        asked_ms: second + 1_000,
        live_fed: false,
        fed_since_ms: None,
        dropped_ms: None,
        barrier: true,
        feed: shadow::Feed::AdditionsOnly,
    };
    assert_eq!(
        shadow::decide(&ask, &view, &facts),
        shadow::Decision::Miss(shadow::Miss::Gap)
    );
}

// ---- what the store's summaries must follow -----------------------------------------------------

fn drain(events: &mut tokio::sync::mpsc::Receiver<WriterEvent>) -> Vec<WriterEvent> {
    std::iter::from_fn(|| events.try_recv().ok()).collect()
}

fn orphan(channel: &str, message: &str, live: Option<&str>) -> WriterEvent {
    WriterEvent::SummaryOrphan {
        channel_id: ChannelId(channel.to_owned()),
        message_id: mid(message),
        live_hash: live.map(crate::summarize::content_hash),
    }
}

fn erased(channel: &str) -> WriterEvent {
    WriterEvent::ChannelErased {
        channel_id: ChannelId(channel.to_owned()),
    }
}

#[test]
fn a_deletion_and_newer_text_are_told_to_the_store_and_nothing_else_is() {
    let fx = Fixture::new("cache-events");
    let mut events = fx.cache.subscribe_events(64);
    fx.list(
        vec![m(1), m(2), m(3), m(4)],
        T0 + DAY_MS,
        Some(newest_proof(ListStart::At(T0))),
    );
    assert_eq!(
        drain(&mut events),
        [],
        "a read of what the cache did not hold"
    );

    // An edit pushed later: the text it says now.
    let edited = Message {
        content: "body number 2, edited".to_owned(),
        ..m(2)
    };
    push(&fx, edited.clone(), true, T0 + DAY_MS + 1);
    // The same text again, and a snippet of a read's text: nothing changes, nothing is told.
    push(&fx, m(3), true, T0 + DAY_MS + 2);
    let cut = Message {
        content: "body…".to_owned(),
        ..m(4)
    };
    push(&fx, cut, true, T0 + DAY_MS + 3);
    // Deletions: of a row held, and of one never held.
    push_delete(&fx, "m1", T0 + DAY_MS + 4);
    push_delete(&fx, "m99", T0 + DAY_MS + 5);
    assert_eq!(
        drain(&mut events),
        [
            orphan("C1", "m2", Some("body number 2, edited")),
            orphan("C1", "m1", None),
            orphan("C1", "m99", None),
        ]
    );

    // A later exhaustive read that lacks m3: gone upstream, and told so. It carries m2 as edited,
    // which tells nothing new.
    fx.clock.set(T0 + 2 * DAY_MS);
    fx.list(
        vec![edited, m(4), m(40)],
        T0 + 2 * DAY_MS,
        Some(newest_proof(ListStart::At(T0))),
    );
    assert_eq!(drain(&mut events), [orphan("C1", "m3", None)]);
    let stats = fx.cache.stats();
    assert_eq!((stats.orphans_sent, stats.orphans_dropped), (4, 0));
}

#[test]
fn an_erased_channel_is_told_once_and_a_channel_the_file_never_named_is_not() {
    let fx = Fixture::new("cache-events-erase");
    let mut events = fx.cache.subscribe_events(64);
    let other = ChanKey::new("p", "C2");
    fx.list(vec![m(1)], T0, None);
    // Coverage, and no rows: still a channel the file names.
    fx.offer(
        &other,
        Observation::Listing(Listing {
            scope: Scope::Main,
            rows: Vec::new(),
            observed_ms: T0,
            proof: None,
        }),
    );
    fx.offer(
        &other,
        Observation::Deleted {
            ids: vec![mid("m50")],
            observed_ms: T0,
        },
    );
    assert_eq!(drain(&mut events), [orphan("C2", "m50", None)]);
    fx.cache.retain_only(RouteSnapshot::new(
        [ChanKey::new("p", "C3")],
        ["p".to_owned()],
    ));
    fx.settle();
    let mut told = drain(&mut events);
    told.sort_by_key(|event| format!("{event:?}"));
    assert_eq!(told, [erased("C1"), erased("C2")]);
    fx.cache
        .erase_channels(&[ChanKey::new("p", "C3"), fx.chan.clone()]);
    fx.settle();
    assert_eq!(
        drain(&mut events),
        [],
        "nothing left to erase, and nothing told"
    );
}

#[test]
fn a_full_queue_drops_events_and_counts_them_and_a_file_erase_tells_nothing() {
    let fx = Fixture::new("cache-events-full");
    let mut events = fx.cache.subscribe_events(1);
    fx.list(
        vec![m(1), m(2), m(3)],
        T0,
        Some(newest_proof(ListStart::At(T0))),
    );
    push_delete(&fx, "m1", T0 + 1);
    push_delete(&fx, "m2", T0 + 2);
    let stats = fx.cache.stats();
    assert_eq!((stats.orphans_sent, stats.orphans_dropped), (1, 1));
    assert_eq!(drain(&mut events), [orphan("C1", "m1", None)]);
    fx.cache.erase_file(true).expect("erased and reopened");
    fx.settle();
    assert_eq!(
        drain(&mut events),
        [],
        "erasing the file says nothing about any message"
    );
}
