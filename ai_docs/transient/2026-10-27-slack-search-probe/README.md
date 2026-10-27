# slack-search-probe: can we use classic `search.messages`, and how good is Real-Time Search?

2026-10-27, `#596 server-search-prep` (for `#521 server-side-search`). The owner asked: "get to
the bottom of whether or not we can use that classic search.messages. Even a single word-based
search should help us find a superset of the messages targeted by our search logic."

Everything here is read-only. The calls were made as the owner, with the owner's stored Slack user
token, through the same command-line Slack client and access path that the private vibe-talk Slack
instance's shim uses. Channel ids, host names, the client's name and all message text are left out.
"The channel" is the busiest channel the harness's chat bridge uses. The scripts are in `scripts/`,
and they name the client through environment variables.

## Answer

**Classic `search.messages` is not usable on our path.** Three independent things stop it, and any
one would be enough:

1. **The client has no command for it.** Its only message search is Slack's Real-Time Search API
   (`assistant.search.context`), and it has no "call any Web API method" command.
2. **The owner's tokens lack the scope.** `search.messages` needs the classic `search:read` user
   scope. Both of the owner's stored user tokens hold only the granular Real-Time Search scopes
   (`search:read.public`, `.private`, `.im`, `.mpim`, `.files`, `.users`), and so do the user
   scopes of both apps those tokens belong to.
3. **The access path does not admit it for automated callers.** Its generic Web API passthrough
   refuses `search.messages` from agents.

Changing any of those is not ours to do, and it would buy little:

- **Same syntax.** Slack documents the same search modifiers for both APIs.
- **The same miss.** Both are word searches, so neither finds a term inside a longer word.

The rest of this record measures Real-Time Search ("RTS").

**RTS is usable as a candidate source, and only for whole words.** It finds messages where the
term stands as a word, or as a common inflection of one (`reports` for `report`). It misses the
term inside a longer word (`able` in "available"). It also misses the term as the start of a
longer word (`coord` in "coordinator"), and `coord*` does not help. Nothing word-based can be a
superset of our substring matches.

## Recall against a ground truth

**Ground truth** (`scripts/ground_truth.py`):

- every message of the channel created on 2026-10-26 (UTC): 216 messages, 32 top-level and 184
  replies, with a median of 656 characters;
- read exhaustively through `conversations.history` and `conversations.replies` (70 calls,
  p50 1.7 s a call);
- every root up to a day older was read too, to catch late replies. No RTS hit fell outside the
  ground truth.

The server's message cache is built from these same reads. It was not read directly, because the
rules for this work keep agents off the live host.

"Ours" is the shared language's rule (`vibe-talk/src/search_query.rs`): the folded term is a
substring of the folded author and text. "Upstream" is every hit of a time-sorted RTS query
narrowed to the channel and the day, paged to the end (`scripts/rts_recall.py`). A missed message
is classed by where the term sits in it:

- **inside**: only inside longer words;
- **start**: at the start of a longer word, but never as a word on its own.

| Term | Ours | Upstream | Both | Recall | Upstream, not ours | Missed: inside / start |
|---|---:|---:|---:|---:|---:|---|
| *pass 1: ten words picked mechanically (alphabetic, 4+ letters, in 5–40 messages, evenly spread)* | | | | | | |
| able | 73 | 5 | 5 | 0.07 | 0 | 68 / 0 |
| metadata | 5 | 6 | 5 | 1.00 | 1 | 0 / 0 |
| bullet | 11 | 11 | 11 | 1.00 | 0 | 0 / 0 |
| asked | 7 | 15 | 7 | 1.00 | 8 | 0 / 0 |
| weird | 7 | 7 | 7 | 1.00 | 0 | 0 / 0 |
| updated | 8 | 21 | 8 | 1.00 | 13 | 0 / 0 |
| queued | 10 | 12 | 10 | 1.00 | 2 | 0 / 0 |
| behind | 13 | 14 | 13 | 1.00 | 1 | 0 / 0 |
| servers | 17 | 60 | 17 | 1.00 | 43 | 0 / 0 |
| report | 36 | 37 | 36 | 1.00 | 1 | 0 / 0 |
| *pass 2: the ten words of the 2026-10-25 pilot* | | | | | | |
| coordinator | 140 | 140 | 140 | 1.00 | 0 | 0 / 0 |
| gate | 14 | 9 | 9 | 0.64 | 0 | 5 / 0 |
| rebase | 0 | 0 | 0 | — | 0 | 0 / 0 |
| landed | 14 | 39 | 14 | 1.00 | 25 | 0 / 0 |
| review | 13 | 12 | 12 | 0.92 | 0 | 1 / 0 |
| agent | 161 | 160 | 160 | 0.99 | 0 | 0 / 1 |
| coord | 143 | 24 | 24 | 0.17 | 0 | 0 / 119 |
| handoff | 3 | 3 | 3 | 1.00 | 0 | 0 / 0 |
| codex | 17 | 18 | 17 | 1.00 | 1 | 0 / 0 |
| slot | 15 | 13 | 12 | 0.80 | 1 | 3 / 0 |
| **all 20** | **707** | **606** | **510** | **0.72** | **96** | **77 / 120** |

95 calls in all: pass 1 took 25 calls (p50 1.9 s) with no rate limit; pass 2 took 70 calls and
waited out 22 rate limits.

**What the numbers say:**

- **Every miss had the term inside a longer word or as its start.** That is 197 of 707 matches,
  28%. Of the misses at a word's start, 119 were `coord`, mostly in "coordinator…". One more
  (`agent`) was not inside a word, and its exact shape was not recorded. Where the term stood as a
  word on its own, or with a common inflection (`bullets`, `reports`), RTS found it. That is better
  than the 2026-10-25 pilot, which saw 2–5 word-start misses per term on another channel; the
  reason for the difference is not known.
- **Its extras are stems.** `servers` also finds "server", `landed` finds "land", and `updated`
  finds "update". They are harmless for a candidate source, because our matcher re-checks every
  hit.
- **Word-aligned is a property of each message, not of the query.** `coord` is three letters and
  passes every shape check a query can make, and still recalls 17%. So the candidate plan
  (`upstream_plan` in `src/search_query.rs`; `searchUpstreamPlan` on the page) treats every
  upstream hit as a candidate:
  - it sends only terms of 3+ letters or digits, as the owner asked on 2026-10-27;
  - an alternative with no such term reports `not-word-aligned`, so the server scans the cache for
    it;
  - and nothing upstream ever moves the "searched back to" horizon (design §4). Only a scan of
    what the server holds, or an exhaustive read, can say a message is absent. The status line
    should say upstream finds whole words only.

## Syntax, re-checked (`scripts/rts_syntax.py`)

One RTS query each, narrowed to the channel and the day unless it names the channel itself. "Ours"
applies the shared language where it can express the query. For `after:`, `before:` and `on:` it
applies Slack's apparent reading (the day itself excluded by `after:`), with days in UTC.

| Query | Ours | Upstream | Reading |
|---|---:|---:|---|
| `gate` | 14 | 9 | the 5 misses are "investigate", "delegate"… (inside a word) |
| `"the gate"` | 1 | 1 | phrases work |
| `in:#<channel> gate` (no channel id) | 14 | 9 | `in:#name` narrows exactly as the id does |
| `gate after:2026-10-26` | 0 | 0 | `after:` **excludes** its day; the window ends on the 27th (UTC) |
| `gate before:2026-10-27` | 14 | 9 | `before:` works |
| `gate on:2026-10-26` | 14 | 8 | `on:` works; the day is probably the owner's zone, not UTC |
| `gate report` | 3 | 10 | **not a strict AND**: it returns messages lacking one of the words |
| `gate OR report` | 47 | 10 | no OR: the same 10 as `gate report` |
| `gate -report` | 11 | 7 | exclusion works (with the inside-a-word misses) |
| `gat*` | 9 | 0 | **the `*` wildcard does nothing through this API** |
| `gat` | 0 | 0 | |
| `coord` | 8 as a word on its own (143 as a substring) | 24 | whole words and close forms only |
| `coord*` | 143 | 24 | the same as `coord` |
| `andoff` | 3 | 0 | no match inside a word |
| `handoff` | 3 | 3 | |
| `from:<@owner>` | — | 10 a page, more pages | `from:` works with a user id, and every hit has that author |
| `gate from:<@owner>` | 0 as a word | 0 | consistent: the owner's 3 substring matches of `gate` are all inside longer words |

## Limits, paging and cost

- **Paging.** One call is one page of about 10 results in time order (the client asks for 20).
  Slack serves at most 20 pages per query, and the 20th page's token returns an empty page with
  `source_complete: false`. Every query here finished below that cap with `source_complete: true`.
  To read more, split the window with `--created-before`.
- **Latency.** 1.7–1.9 s a call (p50) for search and history alike.
- **Rate limits.** Reads and searches are limited separately, and the limits are shared with
  everyone using the same Slack app, the live vibe-talk instance and the chat bridge included.
  - The history walk, at one call per 3 s, was refused 17 times in 70 calls; each refusal waited
    at least 30 s.
  - Search was refused 22 times in pass 2's 70 calls, and Slack itself once answered `429` with
    "retry after 15 seconds".

  Any upstream job must be paced and must back off. The design's "stop on the first rate limit" is
  the right default. Probing like this competes with the owner's live instance for the same
  budget.

## What is not verified

- How soon Slack indexes a new message for search (unmeasured; it needs a post).
- Whether recall holds on other channels and longer windows: one channel-day here, one young
  channel in the pilot.
- Whether file-only messages match. None were in the window.
- How RTS treats accents. The candidate plan sends folded terms (`cafe` for `Café`), and whether
  Slack then finds "café" is unmeasured.
- The live cache's own copy: the ground truth is Slack's history read the way the cache reads it,
  not the cache file.
