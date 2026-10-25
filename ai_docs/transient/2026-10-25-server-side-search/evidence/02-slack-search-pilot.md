# Evidence 2: what Slack's search actually does, measured read-only

2026-10-25 ~22:10–22:30 UTC, `#521 server-side-search`. All calls were reads, made as the owner
through a vendor CLI that wraps Slack's Web API (`conversations.history`) and Slack's Real-Time
Search API (`assistant.search.context`). The private vibe-talk Slack instance reaches Slack the
same way, through its private shim. Channel A is the channel of the "112 messages" summary (evidence 1); channel
B its predecessor; channel C the busiest bridged channel. Ids, host names and message text are
left out; the scripts are under `scripts/`.

## 1. Query language (channel A, `--channel-id` narrowing, ground truth from the cache copy)

"Ours" approximates the page's rule over the same 137 messages: the term as a lower-case substring
of the cache's `search_text` (author + raw content + the renderer's drawn text + link addresses,
capped at 8 KB). That is a little wider than the page's haystack, which has no raw content, and it
misses a term past 8 KB; no message here is that long. "Hits" is distinct messages the search
returned.

| Query | Hits | Ours | Reading |
|---|---:|---:|---|
| `coordinator` / `Coordinator` | 7 / 7 | 5 | case-insensitive; more than ours (stems or unfurls) |
| `coord` | 42 | 44 | **word-prefix** match: finds "coordinator", "coord3" |
| `coord*` | 42 | 44 | `*` suffix is the same as plain prefix |
| `andoff` (inside "handoff") | **0** | 5 | **no infix match** |
| `oordinat` | **0** | 7 | no infix match |
| `"hand off"` | 1 | 1 | phrases work |
| `"landed on main"` | 0 | 0 | phrase, consistent |
| `landed main` | 17 | 12 | implicit AND (and stemming widens it) |
| `review AND rebase` / `review rebase` | 5 / 5 | | AND explicit or implicit |
| `review OR rebase` | **5** | 46 | **no OR**: read as AND (with "or" a stop word) |
| `coordinator OR handoff` | **0** | 10 | no OR |
| `(coordinator OR handoff)` / `coordinator \| handoff` | 0 / 0 | 10 | no grouping, no `\|` |
| `review -rebase` / `review NOT rebase` | 37 / 37 | | exclusion works (42 − 5) |
| `herdr…` (the bridge app's display name) | 1 | 94 | **author names are not searched** |
| `agent-utils` / `agent` / `utils` | 3 / 19 / 3 | 3 / 21 / 3 | hyphenated words split into tokens |
| `review after:2026-10-24` | 24 | | `after:` works in the query |
| `review before:2026-10-24` | 0 | | `before:` works (channel began 10-23) |
| `review on:2026-10-24` | 18 | | `on:` works |
| `review during:october` | 42 | | `during:` works |

## 2. Is keyword search a superset of our matches? No.

`scripts/compare_sets.py`, hits vs. the page's rule, by message:

| Term | Upstream | Ours | Ours but not upstream | Upstream but not ours |
|---|---:|---:|---:|---:|
| coordinator | 8 | 5 | 0 | 3 |
| gate | 7 | 6 | 0 | 1 |
| rebase | 7 | 6 | 0 | 1 |
| landed | 45 | 26 | **5** | 24 |
| review | 38 | 42 | **4** | 0 |
| agent | 20 | 21 | **2** | 1 |
| coord | 41 | 44 | **4** | 1 |
| handoff | 5 | 5 | 0 | 0 |
| codex | 11 | 10 | 0 | 1 |
| slot | 9 | 9 | 0 | 0 |

Looking at the 15 misses in the private copy, none is an infix match: they are words the search
otherwise matches by prefix ("reviewers" for `review`, "coordinator" for `coord`, the latter in a
38-byte message) and plain whole words standing alone (most in 1–3.6 KB posts). The Real-Time Search
API ranks and trims; it is a candidate generator, **never a proof that a message is absent**.
(The hit counts in the two tables differ by a few for the same term because the two passes were
minutes apart and paged differently; the comparison table is the one with exact sets.)

## 3. The wildcard listing is an exhaustive, time-ordered read of a whole channel, replies included

`'*'` with `--channel-id`, `--sort=timestamp --sort-dir=asc`:

| Channel | Hits | Pages | Seconds | Reported completeness | Against the cache copy |
|---|---:|---:|---:|---|---|
| A | 138 | 14 | 22.4 | `source_complete: true` | 137/137 cached messages present; the 1 extra is newer than the copy (its timestamp is after the copy was taken; not confirmed by a later history read) |
| B | 200 | 21 | 37.2 | `source_complete: false`, "Slack returns at most 20 pages per search query" | (more than 200 messages) |

Pages hold **10** results here (the CLI's help says 20; the service answered `page_size: 10`), and
a query is cut at **20 pages = 200 results**, flagged by `source_complete: false`. A longer
channel is read by splitting the window with a created-before bound. This one read returns thread
replies interleaved by time, which is exactly the "All" order, at about 6 messages a second.
Unproven: this is one channel, two days old. Whether a file-only or bot-only message always
matches `*` (the CLI's own help says it did not check; the bridge app's own posts did match); how
soon a new message is indexed; and whether a bot token can make the same call (the pilot ran as
the owner). Treat it as exhaustive only where
a cross-check against history agrees, and keep reading the newest end from history.

## 4. Cost of exhaustive reads (latency per call, p50 over the walk)

| Read | Rows a call | Seconds a call | Channel A | Channel C |
|---|---:|---:|---|---|
| `conversations.history` walk (top-level only) | 20 | 1.6–1.7 | 5 calls, 7.9 s, 82 rows | 6 calls, 10.9 s, 101 rows (+401 replies in 88 threads not read) |
| + `conversations.replies` per thread | ≤ 20 | ~1.6 (est.) | 31 more calls (≈ 50 s) | 88 more calls (≈ 140 s) |
| wildcard search listing (all messages) | 10 | 1.6 | 14 calls, 22.4 s | ≈ 51 calls ≈ 82 s in 3+ windows (est.) |
| keyword search, one page | 10 | 1.5–2.2 (3.5 max) | | |

The vibe-talk server asks `conversations.history` for up to 61 rows; the shim forwards the limit
and the CLI answers 20, so one server page of 50 top-level messages is 3 upstream calls (also
what `tests/slack_flat_calls.rs` counts).

## 5. What is not available on this path

- Slack's classic `search.messages` (the Web API method with `count`/`page`, user tokens with
  `search:read`) is not what the CLI calls; it was not exercised. Its documented syntax has the
  same modifiers (`in:`, `from:`, `before:`, `after:`, `on:`, `during:`, `has:`, quotes,
  `-exclusion`) and, like the Real-Time API, no `OR` between words. Not verified here.
- Freshness: how soon a new message becomes searchable was not measured (that needs a post).
