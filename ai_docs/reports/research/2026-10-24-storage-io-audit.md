# vibe-talk storage and IO audit

`#490 storage-io-audit`, 2026-10-24. The owner asked whether the new send log (`#471
replication-c5-post-keys`) and the rest of vibe-talk's disk storage and IO are reasonably
efficient in compute and disk space, and whether there is any reason not to use an efficient
binary serialization.

**Short answer.** Yes, it is reasonably efficient: every durable table is bounded, nothing
writes on an idle poll, and the live data is under 1 MB of store plus, where a cache runs,
13 MB of message cache with its WAL. The waste was in what gets written, not in how it is
encoded. Three findings matter, and the first two are fixed here:

1. **The send log stored every post's first message twice.** That copy was 39% of each record.
   Fixed: 2683 → 1634 bytes a record.
2. **The server log was 40% ANSI colour codes,** because it coloured output even when writing
   to a file. Fixed: an access line is 300 → 171 bytes.
3. **The message cache's full-text index is 57% of the cache file, and nothing in production
   reads it yet.** The search queries in `cache/search.rs` were written ahead of the search
   milestone, and no route calls them. Every message still pays to be indexed. `detail=full` on
   a trigram index costs 6.5 times what `detail=none` costs. Where the 256 MB ceiling binds, the
   cache holds about half the history it could. Where the age limit binds first, as on the Slack
   deployment, it costs only disk. This is proposed for the owner, not changed (see P1).

A binary format would not materially help. For the send log it saves 9% (CBOR) at slower
decoding. The compact formats (postcard, bincode) **cannot read back** these types at all,
because `Message` skips absent fields when it is serialized. Everywhere else, SQLite's typed
columns already do the job.

Deployments are named by provider below. "Google Chat deployment" is the instance holding a
store only. "Slack deployment" is the instance with three Slack channels, a store and a message
cache.

## How this was measured

- **Live files, read-only.** Each database was copied through SQLite's backup API from a
  `-readonly` connection (`sqlite3 -readonly <live> ".backup /tmp/..."`), and every query ran
  against the copy. Sizes come from `dbstat` (pages, payload and unused bytes per table and
  index). Nothing on either host was written except the copy in `/tmp`. The hosts' own
  `/usr/bin/sqlite3` is 3.34.1, which **cannot open these files**: STRICT tables need 3.37. A
  newer CLI was used instead. Operators should know this before reaching for the stock one.
- **Hot paths.** The release build of `main` at `285c0885` ran locally with `--fake-discord`, a
  real store, a message cache, three channels and a 5 s poll. `strace -f -y` was attached for
  each phase, and the trace was summarised per file (fsync, pwrite count and bytes, journal
  open/unlink). The driver and its raw output were kept outside the tree.
- **Serialization.** A scratch crate linking `vibe_talk` encoded and decoded the 20 real
  send-log records from both stores and the 1,048 real `message.extra` values from the cache,
  2,000 rounds each. It used serde_json (today), ciborium (CBOR), postcard 1.1, bincode 1.3 and
  per-row zstd. The page's localStorage cycle was timed in V8 (Node 22) on a 544k-character
  envelope built from served messages.

Volumes for scale: the busiest deployment sees 62 to 446 chat events a day across five channels
(`2026-10-22-side-agent-helper.md`). The Slack deployment's cache gained 1,048 messages in
9.6 days over 4 channels, about 110 a day. Its send log filed 19 posts in 11.6 hours.

## 1. Inventory

### 1.1 Files on disk

| File | Google Chat deployment | Slack deployment | Bound |
|---|---|---|---|
| Store (`*.sqlite3`) | 684 KB, 167 pages, rollback journal | 463 KB, 113 pages | every table count- or age-bounded (below) |
| Message cache | none | 9.1 MB + 4.0 MB `-wal` + 32 KB `-shm` | `max_days` (14 here; default 90) and `max_megabytes` (default 256), DB + WAL + SHM |
| `app.log` | 16.0 MB over 17 days, ~5,000 lines a day | 7.4 MB over 22 days, up to ~6,500 lines a day | **none**: appended by the unit, never rotated |
| Pre-deploy store copies | 58 files, 24.5 MB | 123 files, 24.2 MB | **none**: written by the deploy script, never pruned |

The pre-deploy copies are 35 to 50 times the live data. They come from the deploy tooling, not
this repository, so they are a recommendation for the operator (P6), not a code change.

### 1.2 The store (one SQLite file, `src/store/sqlite.rs`, migrations v1–v21)

`journal_mode = DELETE` (rollback journal), `synchronous = FULL` (the default), `secure_delete =
ON`, `foreign_keys = ON`. One connection behind a mutex, each call on the blocking pool.

| Table | Google Chat (rows / table+index bytes) | Slack | Growth | Retention |
|---|---|---|---|---|
| `summaries` | 744 / 213 KB + 115 KB PK index | 340 / 82 KB + 41 KB | ~70 a day | 2,000 per kind (summaries, thread names), `summary_retain_days` from last use |
| `dismissals` | 371 / 33 KB + 25 KB + 20 KB | 182 / 16 KB + 12 KB + 12 KB | ~30 a day | 10,000 rows, no age limit (deliberate) |
| `thread_hues` (`#427`) | 209 / 29 KB + 25 KB + 12 KB | 224 / 16 KB + 12 KB + 12 KB | one per new thread | 256 live + 4,096 tombstones per channel |
| `post_keys` (`#471`) | 1 / 4 KB | 19 / 74 KB (52 KB payload) | one per send | 24 h and 4,096 rows, both on every write |
| `pins` | 19 / 37 KB | 7 / 16 KB | by hand | 100 per channel |
| `turns` + `conversations` | 39 / 16 KB | 23 / 12 KB | per voice turn | 50 conversations × 1,000 turns, `retain_days` (30) |
| `client_settings` (`#457`) | 8 rows, 195 B of values | 12 rows, 820 B | by hand | 18 known keys (`SYNCED_KEYS`), 64 KB a value |
| `read_marks` | 0 | 0 | per mark | one per channel |
| everything else | one page each | one page each | by hand | small |

Two observations, neither worth a migration on its own:

- **`summaries` keys cost more than the summaries.** The four-text-column primary key (version
  up to 40 B, channel, a message id or `thread-sentence:<id>` of ~69 B, a 16-hex hash) is stored
  in the row and again in its autoindex: about 300 B of key per ~110 B of summary. `WITHOUT
  ROWID` would drop the second copy, about 35% of the table, or 115 KB today and about 0.6 MB
  at the 4,000-row ceiling. See P4.
- **Worst cases are large but age-bounded.** `turns` allows 50 × 1,000 × 8,000 characters, but
  `retain_days` cuts it long before that. The real tables are tiny.

### 1.3 The message cache (its own SQLite file, `src/cache/*`)

WAL, `synchronous = NORMAL`, `auto_vacuum = INCREMENTAL`, a writer batching for 25 ms,
checkpoints from 4 MiB, background FTS merges and incremental vacuum. It is well engineered
for IO. The space goes to the full-text index:

| Object (Slack deployment) | Bytes | Share | Per message |
|---|---|---|---|
| `message_fts_data` (trigram, `detail=full`) | 5,259,264 | 57% | 5.0 KB |
| `message` rows | 3,489,792 | 38% | 3.3 KB |
| … of which `search_text` | 1,452,790 payload | | 1.39 KB |
| … of which `content` | 894,005 | | 0.85 KB |
| … of which `extra` (JSON) | 219,144 | | 0.21 KB |
| indexes, scopes, coverage, threads | ~390,000 | 4% | 0.37 KB |
| **file** | 9,142,272 | | **8.7 KB** |

At 110 messages a day and 14 days, this deployment settles near 13 MB. At the default 90 days
it would be about 85 MB, still under the 256 MB ceiling, so here the age limit binds and the
index costs disk rather than history. The index is the lever (P1). `detail=column` and
`detail=none` both refuse phrase queries and multi-trigram terms in `MATCH` ("phrase queries
are not supported (detail!=full)"), so either one changes the queries as well as the schema:

| Trigram index over the same 1,048 rows | `_data` bytes |
|---|---|
| `detail=full`, as live (5 segments) | 5,259,264 |
| `detail=full`, after `'optimize'` | 3,813,376 (−28%) |
| `detail=column` | 1,814,528 (−65%) |
| `detail=none` | 688,128 (−87%) |

### 1.4 Page-side storage (`web/voice.js`)

localStorage only; no IndexedDB, no Cache API. 48 keys. The large one is the message cache
envelope (`vibe-talk.voice.message-cache`): up to 12 channel scopes, each with up to 120
messages and 120 thread cards, capped at
600,000 characters (1.2 MB in UTF-16). The outbox (`vibe-talk.outgoing-messages`), drafts and
dismissed sends (10) are small and bounded. Settings keys are a few bytes each.

## 2. Hot paths

Syscalls per event, measured with strace on a local run of the release build (§ How). "Txn" is
one store write transaction.

| Event | Store | Cache | Log |
|---|---|---|---|
| Idle poll tick, no new messages (60 s, 36 polls) | **0 writes** | **0 writes** | 0 |
| Send a reply (one part) | 1 txn: 3 statements (upsert, age prune, count prune); 4 fsync (journal ×2, DB, directory); journal created and unlinked; ~18 KB to the DB + ~17 KB to the journal | ~1.4 KB WAL, no fsync | 1 line |
| Read a channel (`GET /messages`) | 4 page reads, no writes | ~2 pwrites, ~4 KB WAL (the read's coverage and freshness stamp), no fsync | 1 line |
| Dismiss a message | 1 txn, 4 fsync, 16 KB DB + 17 KB journal | ~0.2 KB | 1 line |
| Mark read | 1 txn, 4 fsync, ~8 KB DB + ~9 KB journal | ~0.3 KB | 1 line |
| Change a synced setting | 1 txn, 4 fsync, ~8.6 KB DB + ~9 KB journal | none | 1 line |

The fsyncs here took a median 0.16 ms and at most 27 ms (btrfs on md RAID). At a few hundred
write events a day the store writes about 5–10 MB a day and fsyncs 1–2 thousand times. The
cache adds about 4 KB of WAL per read; at the Google Chat deployment's 3,500 reads a day that
would be about 15 MB a day, checkpointed in 4 MiB steps without fsync per commit.

Every store statement is prepared afresh (`Connection::execute`, `prepare`, never
`prepare_cached`; 0 of ~120 sites). At this volume that is microseconds a call and not worth
churn.

The `thread_hues` read path does not write: a poll tick that colours nothing new stays a read
(`live.rs`, `ops::hues_for`). The cache's listing writer skips rows it already holds
unchanged (`rows_unchanged`).

**The heaviest IO per event is on the page, not the server.** Every channel read
(`loadTimeline`) and each live arrival calls `saveChannelScope`, which:

1. parses the whole envelope (up to 600k characters) and checks every entry of every scope
   against the wire contract (`readMessageCache`, `validCacheEntry`);
2. re-serialises all of it;
3. writes it back with `setItem`; and
4. reads it back whole to check the write (`storedExactly`).

In V8 on this host the parse and serialise alone took 1.33 ms for a 544k-character envelope.
The validation, `setItem` and read-back are on top of that, and a phone is several times slower; that
part is an estimate, not a measurement. The Google Chat deployment's log shows ~3,500
timeline reads a day across devices. If each one rewrites a 1 MB value, the browser rewrites
megabytes of localStorage for every hour a page is open. See P2.

## 3. Serialization: where JSON is stored, and whether to change it

| Site | Today | Measured | Recommendation |
|---|---|---|---|
| `post_keys.record` (send log) | serde_json of `post_keys::Kept` | 2,683 B a row; enc 2.18 µs, dec 3.67 µs. Without the duplicate `first`: **1,634 B**, enc 1.54 µs, dec 2.63 µs. CBOR: 2,437 B (−9%), dec 7.5 µs (2× slower). postcard 1,881 B and bincode 2,124 B **fail to decode**. Per-row zstd: 720 B but 14 µs to encode | **Keep JSON; drop the duplicate (done).** At ≤4,096 rows for 24 h, the remaining 1.6 KB a row comes to about 6.7 MB at the row cap and is typically 30 KB. A row is bounded only by the request size, since it holds the posted text in the fingerprint and again in the parts. A record a person can read with `sqlite3` is worth more than 10% |
| `message.extra` (cache) | JSON object: timestamp, thread, reactions | 211 B a row; CBOR 170 B (−19%) but decoding 2.3× slower (0.91 → 2.11 µs) | **Keep JSON.** 2% of the file. If it grows, promote fields to typed columns rather than change the encoding |
| `client_settings.value` | opaque text from the page (JSON for some keys) | 12 rows, 820 B | Keep. Opaque to the server by design |
| `summaries.summary`, `turns.text`, `pins` | plain TEXT and typed columns | n/a | Already typed columns |
| Wire (`/reply`, `/timeline`, the stream) | JSON | n/a | Out of scope. The browser parses JSON natively; a binary wire would need a decoder in the page |

**Why postcard and bincode fail.** `Message` marks optional fields
`#[serde(skip_serializing_if = "Option::is_none")]`. That is right for JSON, but a format
without field names then cannot tell which field it is reading. Encoding succeeds and the
bytes are unreadable. Making them work would mean removing those attributes, which the JSON
wire contract (`contract/`) depends on, or keeping a second, parallel set of types for storage.
Neither is worth 30% of 30 KB.

**Typed columns already carry the bulk.** Every high-volume table (the cache's `message`,
`coverage` and `scope_head`, the store's `dismissals`, `thread_hues` and `summaries`) is STRICT
with typed columns. JSON is confined to two low-volume sites, and both stay readable with
`json_extract` in a stock `sqlite3`.

## 4. Inefficiencies

### Fixed here

| # | What | Before | After | Commit |
|---|---|---|---|---|
| F1 | The send log filed `Outcome::Posted` with `first` beside `parts`, and `first` is always `parts[0]` (`ops::post_each`). Only the filed form changes. Older records still read, and a record with no part is refused | 2,683 B a record (real records); 1,627 B (local, one part) | 1,634 B (−39%); 1,044 B (−36%) | `vibe-talk: a settled post is filed with its first part once, not twice` |
| F2 | The log was coloured whatever it was written to. Both deployments append it to a file | 39.7% / 39.2% of log bytes were escape codes; an access line 300 B | coloured only on a terminal, never under `NO_COLOR`; an access line 171 B (−43%) | `vibe-talk: no colour codes in a log written to a file or a journal` |

Compatibility of F1: an **older** binary reading a record filed by the new one finds no
`first`, fails to parse it, and skips it, as `AppState` skips any unreadable record. A rollback
therefore forgets the dedupe answers for posts filed since the upgrade, at most 24 hours of
them: a page that retries one of those posts under the same key may post it a second time. The
effect is the same as a restart without a store, and no worse.

### Proposed for the owner (not changed)

**P1. The cache's full-text index (cache).** The largest win: the index is 57% of the file,
and today no production query reads it. The queries in `cache/search.rs` (`RARE_SQL`,
`RARE_SQL_FLAT`, `THREADS_SQL`, `choose`, `rematch`) are called only by tests and
`tests/cache_bench.rs`. Two ways to take the win, for the owner to choose before the search
milestone lands:

- **`detail=none`**: −87% of the index, −50% of the file, about twice the history under the
  same ceiling where the ceiling binds. Every `MATCH` has to become `search_text LIKE '%term%'`
  on the index, with `%`, `_` and the escape character escaped (`ESCAPE`). That covers
  `RARE_SQL`, `RARE_SQL_FLAT`, `THREADS_SQL` and `fts_query`, and the tests that use `MATCH`
  (`cache/privacy_tests.rs`, `cache/tests.rs`, `tests/cache_bench.rs`). The trigram tokenizer
  answers `LIKE` from the index (checked with `EXPLAIN QUERY PLAN`). Candidates are meant to be
  re-matched in Rust (`rematch`), so recall would be unchanged once search ships. Measured on
  104,800 rows (the real rows ×100, so hit counts are inflated): rare phrase 1.4 ms
  (`detail=full`) vs 4.4 ms (`none` + `LIKE`); newest 50 hits 2.0 vs 3.5 ms; index 353 MB vs
  54 MB. Common terms take the newest-first scan either way.
- **Defer the index** until search ships: stop filling `message_fts` and build it then.

Either is a cache schema step. The FTS table uses external content, so an index change is
rebuilt from the `message` table with no refetch from the provider (`cache/schema.rs`, "an
index change is a rebuild from the content table"). Neither needs an erase. Risk: low for
production behaviour today, since nothing reads the index; medium for the search milestone,
which should be benchmarked with `tests/cache_bench.rs` under the chosen shape.

**P2. Page cache writes (`web/voice.js`).** Store each channel scope under its own key, so a
save rewrites one scope (≤1/12 of the envelope), and keep the envelope's last-written string in
memory instead of re-reading it on every save. Alternatively, coalesce `saveChannelScope` to
one write per idle period, flushed on `pagehide`. Risk: medium. The file is under heavy
concurrent editing, and the quota fallback order (cache gives way to drafts and the outbox) has
to survive.

**P3. Store journal mode.** WAL with `synchronous = NORMAL` would take a write from 4 fsyncs
and a journal create/unlink to none per commit (fsync at checkpoint). `synchronous = FULL`
would leave one. It is **not worth doing at today's volume** (1–2 thousand fsyncs a day,
sub-millisecond each). It would also break the deploy tooling's habit of copying the bare
`.sqlite3` file, since committed data can sit in `-wal`. If it is ever done, the backup step
must use `.backup` or `VACUUM INTO`.

**P4. `summaries` as `WITHOUT ROWID`.** Drops the duplicate key copy, about 35% of the table
(0.6 MB at the ceiling). Small; fold it into the next migration that touches the table rather
than spending a migration on it.

**P5. Rotate `app.log`.** It is the one unbounded file the server produces: after F2 about
0.6 MB a day per host, dominated by one access line per request (deliberate, `src/access.rs`).
Rotate it in the deployment: logrotate with `copytruncate` (the unit uses `append:`), or log to
the journal.

**P6. Prune pre-deploy store copies.** Keep the last few (for example, 5) in the deploy tooling.
Today they are 24 MB on each host against under 1 MB of live data.

**P7. FTS optimize after compaction (cache).** The live index had 5 segments. `'optimize'`
shrank it 28%. The writer's weekly merge exists, so this only matters if P1 is not taken.

## What is not verified

- The phone cost of the page cache cycle. The V8 figure is from a desktop host, and
  localStorage write cost in a real browser was not measured.
- Hot-path syscall counts come from the fake provider. A Slack or Google Chat read has the
  same store and cache code, but its listings carry threads, so the cache writes a little more
  per read.
- The FTS timings used Python's SQLite 3.36 on synthetic duplicates. The shipped build bundles
  SQLite 3.53.2.
