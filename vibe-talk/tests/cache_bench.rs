//! `#208 persistent-cache`, milestone `cache-store`: benchmarks of the message cache, IGNORED by
//! default. They take a minute or two and their numbers mean something only in a release build:
//!
//! ```text
//! cargo test --release --test cache_bench -- --ignored --nocapture --test-threads 1
//! ```
//!
//! What they measure, against the plan's targets:
//!
//! - the search milestone's exact SQL (rare terms through the index, common and short ones by the
//!   newest-first scan, and thread search), on 110k rows in one channel and on 12 channels of 10k,
//!   with p50 and p99 over repeated runs, and with the candidates re-matched in Rust;
//! - the writer's sustained rate, filling those files through `offer` in 25 ms batches;
//! - evicting a tenth of the 110k-row file, with the longest single transaction;
//! - erasing one 10k-row channel.
//!
//! The corpus is this crate's own README, cut into messages of about 270 bytes: prose, not random
//! strings, so trigram frequencies are those of real text. Six messages carry a rare word.

use std::time::{Duration, Instant};

use rusqlite::{named_params, Connection, OpenFlags};
use vibe_talk::cache::search::{self, Path as SearchPath};
use vibe_talk::cache::{
    CacheOptions, ChanKey, Incoming, Limits, Listing, Maintenance, MessageCache, Observation,
    RouteSnapshot, Scope,
};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::threads::MessageThread;

const RARE: &str = "quokkaferret";
const ABSENT: &str = "zzqxjvbw";
const COMMON: &str = "the";
const SHORT: &str = "go";
const RUNS: usize = 200;

fn corpus() -> Vec<String> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))
        .expect("the README");
    text.split_whitespace().map(str::to_owned).collect()
}

fn message(
    id: u64,
    channel: &str,
    words: &[String],
    created_ms: i64,
    thread: Option<u64>,
) -> Message {
    let start = usize::try_from(id.wrapping_mul(7919)).unwrap_or(0) % (words.len() - 60);
    let mut content = String::new();
    for word in &words[start..] {
        if content.len() > 260 {
            break;
        }
        if !content.is_empty() {
            content.push(' ');
        }
        content.push_str(word);
    }
    if id % 18_333 == 7 {
        content.push_str(" a ");
        content.push_str(RARE);
    }
    Message {
        thread: thread.map(|t| MessageThread {
            id: format!("t{t}"),
            root_message_id: None,
            is_root: false,
            reply_count: None,
            reply_count_exact: false,
        }),
        id: MessageId(format!("{channel}-{id}")),
        channel_id: ChannelId(channel.to_owned()),
        author: format!("author{}", id % 37),
        author_id: UserId(format!("U{}", id % 37)),
        author_is_bot: false,
        timestamp: vibe_talk::clock::iso_from_ms(created_ms),
        spoken_time: String::new(),
        reply_to: None,
        content,
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
        heading: None,
    }
}

struct Bench {
    cache: MessageCache,
    dir: vibe_talk::testing::TempDir,
    channels: Vec<ChanKey>,
}

impl Bench {
    fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("cache").join("bench.sqlite3")
    }
}

/// Fill a cache through the writer, `per_channel` rows in each of `channels`, one in five a thread
/// reply. Reports the sustained rate.
fn fill(tag: &str, channels: usize, per_channel: u64) -> Bench {
    let dir = vibe_talk::testing::TempDir::new(tag);
    let cache = MessageCache::open(CacheOptions {
        limits: Limits {
            max_bytes: 8 * 1024 * 1024 * 1024,
            ..Limits::default()
        },
        ..CacheOptions::new(dir.path().join("cache").join("bench.sqlite3"))
    })
    .expect("open");
    let keys: Vec<ChanKey> = (0..channels)
        .map(|c| ChanKey::new("p", format!("C{c}")))
        .collect();
    cache.retain_only(RouteSnapshot::new(keys.clone(), ["p".to_owned()]));
    let words = corpus();
    let now = vibe_talk::cache::wall_ms();
    let started = Instant::now();
    let mut rows = 0_u64;
    for (c, chan) in keys.iter().enumerate() {
        let mut i = 0;
        while i < per_channel {
            let batch: Vec<Incoming> = (i..(i + 100).min(per_channel))
                .map(|n| {
                    let created = now - i64::try_from(per_channel - n).unwrap_or(0) * 20_000;
                    let id = n + u64::try_from(c).unwrap_or(0) * 1_000_000;
                    if n % 5 == 0 {
                        let thread = n / 50;
                        Incoming::new(
                            message(id, &chan.channel_id, &words, created, Some(thread)),
                            Scope::Thread(format!("t{thread}")),
                        )
                    } else {
                        Incoming::new(
                            message(id, &chan.channel_id, &words, created, None),
                            Scope::Main,
                        )
                    }
                })
                .collect();
            rows += u64::try_from(batch.len()).unwrap_or(0);
            let stamp = cache.stamp(chan);
            cache.offer(
                chan.clone(),
                stamp,
                Observation::Listing(Listing {
                    scope: Scope::Main,
                    rows: batch,
                    observed_ms: now,
                    proof: None,
                }),
            );
            i += 100;
            // Keep the queue short of its byte bound: this measures the writer, not the drop path.
            if i % 5_000 == 0 {
                assert!(cache.barrier_blocking(Duration::from_secs(120)));
            }
        }
    }
    assert!(cache.barrier_blocking(Duration::from_secs(600)));
    let elapsed = started.elapsed();
    let stats = cache.stats();
    #[allow(clippy::cast_precision_loss)]
    let rate = rows as f64 / elapsed.as_secs_f64();
    eprintln!(
        "[{tag}] writer: {rows} rows in {:.2} s = {rate:.0} rows/s; {} batches, longest {:.1} ms; \
         file {:.1} MB; dropped {}",
        elapsed.as_secs_f64(),
        stats.batches,
        ms(stats.batch_max_us),
        mb(cache.bytes_on_disk()),
        stats.ops_dropped,
    );
    assert_eq!(stats.ops_dropped, 0);
    Bench {
        cache,
        dir,
        channels: keys,
    }
}

#[allow(clippy::cast_precision_loss)]
fn ms(us: u64) -> f64 {
    us as f64 / 1000.0
}

#[allow(clippy::cast_precision_loss)]
fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

fn percentiles(mut samples: Vec<Duration>) -> (f64, f64) {
    samples.sort();
    let at = |q: f64| {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let index = ((samples.len() as f64 - 1.0) * q).round() as usize;
        samples[index].as_secs_f64() * 1000.0
    };
    (at(0.5), at(0.99))
}

fn chan_id(connection: &Connection, chan: &ChanKey) -> i64 {
    connection
        .query_row(
            "SELECT chan FROM channel WHERE provider_key = ?1 AND channel_id = ?2",
            [&chan.provider_key, &chan.channel_id],
            |row| row.get(0),
        )
        .expect("interned")
}

/// Run one term through the shape `choose` picks, `RUNS` times, and with Rust re-matching.
fn search_term(connection: &Connection, chan: i64, term: &str, tag: &str) {
    let terms = vec![term.to_owned()];
    let path = search::choose(connection, &terms).expect("choose");
    let query = search::fts_query(&terms);
    let scan_sql = search::scan_sql(1);
    let run = |shape: SearchPath| -> Vec<i64> {
        match shape {
            SearchPath::Index => {
                let mut statement = connection.prepare_cached_free(search::RARE_SQL);
                statement
                    .query_map(
                        named_params! {":q": query, ":chan": chan, ":from_ms": 0_i64,
                        ":before_ms": i64::MAX, ":limit": 50_i64},
                        |row| row.get(0),
                    )
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<i64>>>()
                    .expect("rows")
            }
            SearchPath::Scan => {
                let mut statement = connection.prepare_cached_free(&scan_sql);
                statement
                    .query_map(
                        named_params! {":chan": chan, ":from_ms": 0_i64, ":before_ms": i64::MAX,
                        ":limit": 50_i64, ":t1": term},
                        |row| row.get(0),
                    )
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<i64>>>()
                    .expect("rows")
            }
        }
    };
    for shape in [SearchPath::Index, SearchPath::Scan] {
        if shape == SearchPath::Index && term.chars().count() < 3 {
            continue;
        }
        let mut samples = Vec::with_capacity(RUNS);
        let mut hits = 0;
        for _ in 0..RUNS {
            let started = Instant::now();
            hits = run(shape).len();
            samples.push(started.elapsed());
        }
        let (p50, p99) = percentiles(samples);
        let chosen = if shape == path { " (chosen)" } else { "" };
        eprintln!(
            "[{tag}] {term:>12} {shape:?}{chosen}: {hits} hits, p50 {p50:.3} ms, p99 {p99:.3} ms"
        );
    }
    // The chosen shape end to end: the choice, the SQL, and every candidate matched again in Rust.
    let mut samples = Vec::with_capacity(RUNS);
    let mut fetch = connection
        .prepare("SELECT author, content FROM message WHERE row_id = ?1")
        .expect("fetch");
    for _ in 0..RUNS {
        let started = Instant::now();
        let shape = search::choose(connection, &terms).expect("choose");
        let candidates = run(shape);
        let mut kept = 0;
        for row_id in candidates {
            let (author, content): (String, String) = fetch
                .query_row([row_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("row");
            if search::rematch(&format!("{author}\n{content}"), &terms) {
                kept += 1;
            }
        }
        assert!(kept <= 50);
        samples.push(started.elapsed());
    }
    let (p50, p99) = percentiles(samples);
    eprintln!("[{tag}] {term:>12} with re-matching: p50 {p50:.3} ms, p99 {p99:.3} ms");
}

fn search_threads(connection: &Connection, chan: i64, term: &str, tag: &str) {
    let query = search::fts_query(&[term.to_owned()]);
    let mut samples = Vec::with_capacity(RUNS);
    let mut threads = 0;
    for _ in 0..RUNS {
        let started = Instant::now();
        let mut statement = connection.prepare_cached_free(search::THREADS_SQL);
        threads = statement
            .query_map(named_params! {":q": query, ":chan": chan}, |row| {
                row.get::<_, String>(0)
            })
            .expect("query")
            .count();
        samples.push(started.elapsed());
    }
    let (p50, p99) = percentiles(samples);
    eprintln!("[{tag}] threads {term:>12}: {threads} threads, p50 {p50:.3} ms, p99 {p99:.3} ms");
}

/// `Connection::prepare` under a name that says why it is not cached: this crate builds rusqlite
/// without its statement cache, and the benchmark should pay what a route would.
trait PrepareFree {
    fn prepare_cached_free(&self, sql: &str) -> rusqlite::Statement<'_>;
}

impl PrepareFree for Connection {
    fn prepare_cached_free(&self, sql: &str) -> rusqlite::Statement<'_> {
        self.prepare(sql).expect("prepare")
    }
}

fn reader(path: &std::path::Path) -> Connection {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("reader");
    connection
        .execute_batch("PRAGMA temp_store = MEMORY; PRAGMA query_only = ON;")
        .expect("pragmas");
    connection
}

#[test]
#[ignore = "a benchmark: run by hand in release mode"]
fn bench_one_channel_of_110k_rows_search_and_eviction() {
    let bench = fill("bench-110k", 1, 110_000);
    let connection = reader(&bench.path());
    let chan = chan_id(&connection, &bench.channels[0]);
    for term in [RARE, ABSENT, COMMON, SHORT] {
        search_term(&connection, chan, term, "110k");
    }
    for term in [RARE, COMMON] {
        search_threads(&connection, chan, term, "110k");
    }
    drop(connection);

    // Evict a tenth: reopen with the limit at the pages now in use, so the target (90 % of it) is
    // a tenth below.
    let path = bench.path();
    let Bench { cache, dir, .. } = bench;
    drop(cache);
    let used = {
        let c = reader(&path);
        let pages: i64 = c
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .expect("pages");
        let free: i64 = c
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))
            .expect("free");
        let size: i64 = c
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .expect("size");
        u64::try_from((pages - free) * size).expect("size")
    };
    let cache = MessageCache::open(CacheOptions {
        limits: Limits {
            max_bytes: used,
            ..Limits::default()
        },
        ..CacheOptions::new(&path)
    })
    .expect("reopen");
    let before = cache.counts().served().expect("counts").messages;
    let started = Instant::now();
    cache.maintain(Maintenance::Bounds);
    assert!(cache.barrier_blocking(Duration::from_secs(600)));
    let elapsed = started.elapsed();
    let after = cache.counts().served().expect("counts").messages;
    let stats = cache.stats();
    eprintln!(
        "[110k] eviction to 90 % of {:.1} MB: {} of {before} rows in {:.3} s; {} chunks, longest \
         {:.1} ms; compaction {:.1} ms, longest slice {:.1} ms; file now {:.1} MB",
        mb(used),
        before - after,
        elapsed.as_secs_f64(),
        stats.eviction_chunks,
        ms(stats.eviction_chunk_max_us),
        ms(stats.compaction_max_us),
        ms(stats.compaction_slice_max_us),
        mb(cache.bytes_on_disk()),
    );
    drop(cache);
    drop(dir);
}

#[test]
#[ignore = "a benchmark: run by hand in release mode"]
fn bench_twelve_channels_of_10k_rows_search_and_erase() {
    let bench = fill("bench-12x10k", 12, 10_000);
    let connection = reader(&bench.path());
    let chan = chan_id(&connection, &bench.channels[5]);
    for term in [RARE, ABSENT, COMMON, SHORT] {
        search_term(&connection, chan, term, "12x10k");
    }
    for term in [RARE, COMMON] {
        search_threads(&connection, chan, term, "12x10k");
    }
    drop(connection);
    let started = Instant::now();
    bench
        .cache
        .erase_channels(std::slice::from_ref(&bench.channels[5]));
    assert!(bench.cache.barrier_blocking(Duration::from_secs(600)));
    let elapsed = started.elapsed();
    let stats = bench.cache.stats();
    eprintln!(
        "[12x10k] erase of one channel: {} rows in {:.3} s; compaction {:.1} ms, longest slice \
         {:.1} ms; {} left",
        stats.erased_rows,
        elapsed.as_secs_f64(),
        ms(stats.compaction_max_us),
        ms(stats.compaction_slice_max_us),
        bench.cache.counts().served().expect("counts").messages,
    );
}
