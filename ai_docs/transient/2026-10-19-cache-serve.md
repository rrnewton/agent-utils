# cache-serve: answering reads from the persistent message cache

`#208 persistent-cache`, milestone M9. Design note, 2026-10-19. The cache has
filled and measured in shadow mode since M5 (`cache-fill`); `#297
cache-coverage-races` closed the races that blocked serving and left
requirements (a)–(h) for this milestone. This note says which reads M9 serves,
what the reader sees meanwhile, how a served answer is corrected, and how each
of (a)–(h) is met or the read is not served.

## The problem

A page reload reads the provider afresh. On a slow provider (a bridge whose
whole-space read takes 20–50 s, worsened when two servers share one account's
read limit) the reader sees "Loading messages…", then "did not answer within
20 seconds", and only later the messages — although the server already holds
them on disk in the message cache.

## Switch

One new key under the existing table: `[message_cache] serve`, default
`false`. With `serve = false`, an absent table, or the owner's switch off,
behaviour is exactly today's: fill and shadow-measure, answer nothing. Serving
additionally needs `enabled = true` and an open cache. Only **write-scope**
callers are served in this milestone: the page is one, and the background
refresh below then fills under the caller's own grant, so the rule "a
read-scope request writes nothing durable" is untouched. A read-scope request
is still shadowed, as today.

## Which reads may be served

A read is served only when the shadow rule (`cache::shadow::decide`, the rule
the counters have measured since M5) decides `Serve` for its ask on a snapshot
read at ask time, barrier included. M9 serves two families:

1. **The newest timeline page** (`GET …/timeline` with neither `before` nor
   `after`) — the page-load and reload path — in the **Main** view (any
   provider shape) and the **Flat** view on a bridge. The timeline rule needs
   no fresh head: coverage contiguous from the head, band vouched, nothing in
   doubt, nothing held only as a push brought it. A served page is therefore
   *provisional*: correct as of the last listing the cache holds, however old,
   and always followed by a background refresh (below).
2. **A channel's newest window** (`ops::messages` and its callers: digest,
   to-do, bankruptcy, read-aloud preparation) under the window rule, which
   *does* require a fresh head and current claims. Such an answer is as fresh
   as the recent-window reuse the server already practises, so it is handed
   back as a final answer for that request; a background refresh still runs so
   the served answer is checked against upstream (below).

Never served in this milestone, and still shadowed so the numbers keep
coming: a message by id and the message a summary is of (their
existence-check semantics are a later decision), `/page`, `/count`, a `before`
or `after` timeline read, the thread-list view, one thread's timeline page
(its root and summary cannot be rebuilt from rows alone yet), Slack's Flat
view, and **a Slack window** (requirement (c)). `timeline_fallback_ms` stays
read-and-unused: with `serve` on, an eligible newest page is answered at once
rather than after a wait, which is the behaviour the owner asked for.

## What the page shows while the refresh runs

The served page is an ordinary timeline page: the cached messages, stamped
and marked exactly as upstream ones, so the reader sees messages immediately
instead of "Loading messages…". Three fields say it is provisional, all of
which the page already handles for other providers:

- `as_of` is the cached head's instant: the page is complete only up to then.
- `next_after` is a generic forward cursor from the served page (what the
  server mints whenever a backend issues none), never a backend cursor the
  cache cannot know.
- `next_before` is absent and `has_more` is true unless the cache's coverage
  proves the scope's start: the page already reads that as "more above:
  unknown". Stepping back from a provisional page waits for the next full
  read; everything else (reading, replying, live updates) works at once.

## How a served answer is corrected

Every serve starts **one background refresh**: the same upstream read the
request would have waited for, coalesced per channel and read, under the
caller's grant. When it lands:

1. Its answer is compared with the snapshot that was served, through the
   existing shadow counters (`compared`, `agreed`, `missing_rows`,
   `extra_rows`, `content_differs`), so a served answer is checked against
   upstream exactly as a shadowed one is.
2. It is offered to the cache as any read is: edits replace, deletions
   tombstone, summaries of old text are orphaned, all by the existing rules.
3. For each message that was **actually served** and that the cache, after
   the refresh has been applied, now holds differently — edited text, or no
   longer held at all — the server publishes a live correction (update or
   delete) for that message. The page already applies live updates and
   deletions to rows on screen, so a wrong row is put right without waiting
   for the next poll; the next ordinary read is correct in any case. The
   cache is the arbiter, so a refresh copy that is itself stale (a bridge's
   `as_of` page) cannot "correct" a row backwards. New messages are not
   published by the refresh: they arrive by the live feed, a delta or the
   poll, as they do after an ordinary page load today.

A refresh that fails leaves the provisional page standing and is logged with
counts only; the next poll or read re-checks upstream. Nothing about a serve
suppresses the ordinary read paths.

## The #297 requirements

- **(a) A refused erase's block is memory-only.** Unchanged, and bounded for
  serving: the live feed vouches for nothing from before a start (as the
  README already says), and every serve's refresh re-reads the scope at once,
  so a refusal is heard again within one refresh and the existing
  block-and-erase machinery runs. Until that refresh lands, a restarted
  server could show rows of a channel whose access was lost before the
  restart to the write-token reader who already saw them; it cannot show
  them past that refresh.
- **(b) Two newest reads begun in the same millisecond.** Not fixed here.
  Its worst case under serving is a just-deleted band row shown until the
  refresh's delete correction, and counted as an extra row when it happens.
- **(c) Slack windows from Main plus thread scopes, or not at all.** Not at
  all: a Slack-shaped channel's window is never served in M9 (its upstream
  window carries thread replies the cache holds only in their threads). A
  Slack *Main timeline page* is the conversation's own history, which is
  exactly the Main scope, so it may be served. Serving Slack windows from
  Main plus the active threads' scopes is later work.
- **(d) No deterministic test for the pending-entry ordering.** Serving adds
  no new ordering surface: a serve reads through the same epoch-checked
  reader, and a pending erase blocks the read (`blocked`), so a serve cannot
  slip past an erase the fill could not.
- **(e) Flat-memory Slack replies.** Rows whose text comes from Flat's memory
  are thread replies. M9 serves no read whose rows are thread replies from
  that memory: no Slack window (c), and no thread-view timeline page for any
  shape. A reply also sent to the channel appears in Slack's Main timeline
  from the conversation history read itself, not from Flat's memory.
- **(f) A whole-channel erase's drop is memory-only until the next row.**
  As (a): after a restart the feed vouches for nothing from before it, a
  bridge copy dated before the erase is served only within
  `REVALIDATED_BRIDGE_MS` of its date by the message rule (not served at all
  in M9), and a served window or page rests on coverage claims, which the
  refresh re-lists at once. The provisional-page bound in (a) is the honest
  residue.
- **(g) "Fed" judged per provider.** The sink now records, per provider, when
  an adapter last pushed a change that reached it. A channel's feed vouches
  (in the shadow facts, for serving and for counting alike) only when its
  provider is not polled *and* an adapter has pushed for that provider within
  the last hour by the cache's clock. An adapter that never connects, or has
  been silent for over an hour, leaves the channel to the freshness rules
  alone. There is no heartbeat in the ingest protocol; one hour is the
  chosen bound, stated here so it can be argued with.
- **(h) Pushed snippet edits leave a channel in doubt.** Already the rule: a
  channel in doubt decides `Miss(Suspect)` and is not served, and the
  shadow's `suspect` count per consumer is the number to watch on such a
  feed before turning `serve` on for it. M9 adds a `served` count beside
  `would_serve` so the two can be compared.

## Tests and measurement

New integration tests (`vibe-talk/tests/message_cache_serve.rs`) fail before
the change: with the fake provider's timeline read held, a reload is answered
from the cache within a second while the provider has not answered at all; an
edit and a deletion made upstream while the refresh is held correct the
served page by live events once the refresh lands, and the shadow comparison
counts them; a channel in doubt (a drop recorded after its rows were seen) is
not served and waits for upstream. Reload time to first messages is measured
with the held fake: before, unanswered after two seconds (the test's bound;
the real provider took 20 s and more); after, the elapsed time of the served
request, printed by the test run and reported with the work.
