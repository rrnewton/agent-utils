# Appendix 4: vibe-talk on `logbook`

Part of `#449 wrkviz-vibe-talk-synthesis`. How vibe-talk's store, message cache and summarizers move
onto the shared crate without losing a guarantee, in phases that can each be deployed and rolled
back on their own.

## What moves and what stays

| Today | After |
|---|---|
| `StateStore` (`src/store/`): read marks, dismissals, pins, bookmarks, aliases, added and hidden channels, noise rules, voice settings and prompt, post prefix, upstream clear, thread hues, voice conversations | **Stays** in vibe-talk's store. It is the owner's own state, not message data, and its migrations, retention and purge are vibe-talk's |
| `summaries` table: per-message one-liners, thread names (kept version) and sentences | **Moves** to `logbook` artifacts: `Unit::Item` and `Unit::Group(ChatThread)`, content-addressed, with receipts. The startup re-key and sweep become artifact selection by version |
| Message cache (`src/cache/`): rows, tombstones, coverage, heads, threads, FTS, writer, sink, access watch, shadow, serve | **Becomes** `logbook`'s `ChatStore` (`Store<ChatMeta>`) for rows, tombstones, threads and FTS; the chat-specific parts (coverage algebra, heads and bands, the shadow and serve rules, access watch, fill grants) move as a `logbook::chat` module that only chat sources use |
| `recent.rs` held windows, `read_ahead.rs` | **Stay** in memory as they are; later read from `ChatStore` when its watermark is fresh |
| `summarize/messages_api.rs` (client, auth dialects, pacing) | **Moves** to `logbook` as the `MessagesApi` `ModelBackend`, shared with wrkviz |
| `summarize/threads.rs` queue and budgets | **Stays** in vibe-talk as the scheduler for thread naming (it reads the provider); the naming call itself goes through `logbook` |
| `summary.rs` `condense` and `digest` | **Stays**: deterministic speech formatting, not storage |

## The durability decision this forces

The message cache is defined as **disposable**: erasing it loses nothing the provider cannot give
back, it is bounded by 90 days and 256 MB, and access loss erases it. Weekly reports, timelines and
summaries want **history**. Two retention classes, chosen per source in configuration, keep both
promises honest:

| Class | Holds | Bound | On access loss | On purge |
|---|---|---|---|---|
| `mirror` (default, today's cache) | Chat rows | Age and size, as today | Erased when confirmed | Erased |
| `archive` (opt-in per channel) | Chat rows the owner chose to keep | Owner-set age, or none | **Frozen, not erased**, and marked; erased only on the owner's command | Erased (the purge stays total) |

Summaries and reports are derived third-party text either way. They follow their sources: a summary
whose input rows are erased is orphaned and deleted, as `cache/orphans.rs` already does for the
store's summaries. A weekly report the owner approved is the owner's document and is kept (question
4 for the owner).

## Guarantees, and where each lands

| Guarantee | Today | In `logbook` |
|---|---|---|
| Read scope writes nothing durable | `FillGrant` only for write-scope callers (`cache/grant.rs`) | `Capability { write: false }` has no path to `Store::apply`; the grant type moves with the chat module |
| Allowlist | Route snapshot; sweep of removed channels | `Capability.sources`; `Erase::Source` on removal (mirror) or freeze (archive) |
| Untrusted text | `untrusted::fenced`, private prompt fields | `Staged::payload: Fenced`, constructible only through the fence; the module moves into `logbook` and vibe-talk re-exports it |
| Owner identity | `[owner]`, "the user (Name)" lines | `Author::Owner` set at ingest from the same configuration; every renderer for a model marks it the same way |
| Noise rules | Evaluated at serve time on current text, never recorded | Unchanged: noise is a serve-time flag. Summarizers call the same predicate at stage time and skip noise items, so a placeholder like `_Working…_` never reaches a model, and freshness counts only summarizable items. Each artifact records a digest of the rules it was staged under, so changing a rule makes the affected summaries `Behind` through that digest (appendix 2) |
| Access loss | Block at once, erase on confirmation | Same machinery for mirror sources; freeze for archive sources |
| Privacy at rest | `secure_delete`, FTS secure-delete, id-and-time tombstones, logs without text | `SqliteStore` opens every connection with the same pragmas; the source test that forbids `INSERT OR` moves with it |
| Single tenant | One owner per store | Unchanged; the transcript store is the owner's too |
| Serve correctness | `shadow::decide`, `serve_eligible`, refresh and correction | Moved unchanged into `logbook::chat`; still the only rule that may answer a page read from storage |
| Purge | `purge_everything` plus the cache lifecycle | One purge erases vibe-talk's store and every `logbook` file the deployment owns, with the existing receipt line |

## Phased cutover

Each phase is shippable alone, keeps today's tests green, and has a switch to go back.

1. **C1: the crate exists, vibe-talk calls it for one thing.** `logbook` with ids, `ChatMeta`,
   `Fenced`, `ModelBackend::MessagesApi` moved out of `summarize/messages_api.rs`. vibe-talk's
   summarizers call it. No storage change. Rollback: none needed (a move).
2. **C2: summaries become artifacts.** New per-message and thread artifacts written to `logbook`;
   the `summaries` table is read as a fallback and copied on first touch (a re-key, as `#208
   summary-keep` did once). Receipts start. Rollback: the table is still there and still written
   for one release.
3. **C3: the cache file becomes `ChatStore`.** The cache schema (`cache/schema.rs`, three steps) is
   already close to `logbook`'s chat tables; a migration step 4 adds the generic columns (item
   family, `observed`, body kind) rather than copying data. The writer, sequencer, readers and
   barrier move as they are. Serve stays off where it is off. Rollback: schema compat value keeps
   the old code able to open the file read-only; the cache is disposable in any case.
4. **C4: backfill and watermarks.** Implement the backfill walk vibe-talk already declares
   (`[message_cache] backfill = "adaptive"`, `ServerFill::Backfill`, `ServerFill::Refresher`, none of
   which has a caller today); it advances `complete_through` per channel (`AsListed` on Slack), and
   tallies and freshness become available. It should also lift the cache's Main-page serve rate. This is what the availability bar and hourly
   summaries need, and it is independent of serving. Rollback: turn the walk off.
5. **C5: hourly and daily chat rollups, topics.** New summarizers (`chat-hour`, `chat-day`) and
   thread topics; the voice context and the bar use them (appendix 8). Rollback: configuration.
6. **C6: the Work tab** (appendix 5) and the transcript store, in the same process or beside it.

## Testing the cutover

- Every existing cache test (`cache/tests.rs`, `race_tests.rs`, `coverage_race_tests.rs`,
  `privacy_tests.rs`, `shadow_tests.rs`) moves with the module and must pass unchanged.
- The equivalence harness used for `#208 persistent-cache`'s cache-fill milestone (every upstream and
  summarizer call identical with the cache absent and disabled, about 1,450 steps over five fakes) is
  imported into the tree (today it lives in a private scratch directory, per `#297
  cache-coverage-races`) and rerun at C1, C2 and C3: moving code must not change a single upstream
  call.
- A summary-continuity test: at C2, a message whose `(policy version, content hash)` row exists in
  the old table resolves to that summary (copied into an artifact marked `legacy_storage`, as
  wrkviz marks its legacy cache hits) with no model call, so no summary is paid for twice at
  cutover.
