# server-side-search: pilot study, design and implementation plan

2026-10-25, `#521 server-side-search`. **A plan for the owner to decide on.** Updated
2026-10-27 (`#596 server-search-prep`): the owner settled questions 3, 4 and 10. §9 compares the
query syntax of five products, §10 evaluates existing crates, and §11 lists the
question-independent groundwork that landed (none of it reachable from the page) and what still
waits on a question. Evidence is in `evidence/`, with the scripts that produced it in `evidence/scripts/`.
Channel ids, host names, the private shim's location and message text are redacted throughout:
"channel A" is the owner's channel whose summary read "112 messages", "channel B" the one before
it, "channel C" the busiest bridged channel. Revised once after an adversarial review against the
code (its findings are folded in; see "Review" at the end).

## The owner's request, in short

The search-older button on Slack's All view said *"Nothing in All matches that search — all of it
is loaded"* under *"112 messages from <channel A>"*. The owner asked:

1. make the counts and the completeness claims accurate;
2. send the same search to the server, which answers from its message cache and then from the chat
   service, and catches the page up with **just the far-back hits**, not contiguous history;
3. one search language, quoting and AND/OR/NOT, giving **exactly** the same results on the page
   and on the server;
4. a trustworthy **"searched back to <date>"** line that only ever moves backwards: loaded rows,
   then the server's cache, then the chat service, until it can say "the whole channel";
5. a pilot of what Slack, Discord and Google Chat can do upstream, before any of it is built.

## Summary

- **The "112 … all of it is loaded" claim is wrong, and not for the reason it looks.** Channel A
  is only two days old (first message 2026-10-23 23:24 UTC) and holds 137 messages: 80 top-level
  and 57 replies in 31 threads. The earlier traffic is in channel B. On Slack, "All" is each
  top-level page plus the replies of **at most 10 threads per page**. Channel A's newest page has
  21 threads, so 23 replies in 11 threads are never loaded by any page. Reaching the start of the
  top-level history is then reported as "the whole of All is loaded". A replay over channel A's
  real shape loads 114 messages; the owner saw 112. Reproduced in a server test.
  ([§1](#1-diagnosis-112-messages--all-of-it-is-loaded), evidence 1.)
- **No chat service's search can stand in for ours.** Slack's search (measured) matches word
  prefixes, never infixes. It has no OR, does not search author names, and dropped 2–5 messages
  per term that contain the exact word. Discord's bot search (since 2026-03) and Google Chat's
  (user auth only) are token-based too, with no OR. So upstream search can only give **hints**,
  which we check again here. Every "searched back to" claim has to rest on an **exhaustive,
  time-windowed read**. ([§2](#2-what-each-chat-service-can-do-upstream), evidence 2–3.)
- **Slack has a read that fits All exactly, and looks exhaustive.** A wildcard search narrowed to
  one channel and sorted by time listed every message, replies included, in time order: all 137
  cached messages of channel A (plus one newer), about 6 messages a second, at most 200 per query
  (longer channels split by time). One young channel is not proof, so the plan cross-checks it
  against history before trusting it; it can then feed the server's search backfill, and All's
  older pages.
- **Recommended single source of truth: one written spec plus a shared conformance corpus**, run
  against a Rust and a JavaScript matcher in CI, with a cross-language differential fuzz. Not WASM.
  The matcher is the small half (~300 lines a side). The large half is *what text is searched*,
  and WASM does not solve that. ([§5](#5-one-matcher-two-languages).)
- **2026-10-27 (`#596 server-search-prep`):**
  - Classic `search.messages` cannot be called on our Slack path: no command for it, no
    `search:read` scope, and the path refuses it.
  - Real-Time Search found 72% of our substring matches over 20 terms on one channel-day, with
    whole words found and everything inside or at the start of a longer word missed
    (`ai_docs/transient/2026-10-27-slack-search-probe/`).
  - The settled syntax, its precedents (§9), a crate evaluation (§10) and the groundwork that
    landed (§11) are below.
- **Plan:** five phases, about **10–15 agent-days**. Phase 0 (honest counts and completeness) is
  under a day and independent of the rest. ([§7](#7-phased-plan).) Ten questions for the owner are
  in [§8](#8-questions-for-the-owner).

---

## 1. Diagnosis: "112 messages … all of it is loaded"

**What the numbers mean today.** "112 messages from <channel>" is `channelSummary(count, whole,
label)` in `web/voice.js`. `count` is the messages loaded in this view, less those hidden as empty
and, under Hide read, those read; All's projection also includes replies of threads the reader
opened. The server counts nothing. The page words it as a total ("N messages from X") whenever
`loadedIsWhole()` (which reads only `discordMoreAbove`) says the view reaches the start, and
otherwise as "the most recent messages; older ones are not loaded". "Nothing in All matches that
search — all of it is loaded" is the empty-search sentence (`searchEmptySentence`), shown when
`historyReadToStart()` is true. (On a provider without threads under Hide read the same sentence
can appear for another reason: that list sets `discordMoreAbove = false` itself.)

**Why the page believed it.**

1. Slack's All (`HttpSlackClient::flat`, `src/slack/timeline.rs`) is Main's page of 50 top-level
   messages plus the replies of at most `FLAT_MAX_THREADS` = 10 of the threads rooted on it (the
   most recently active), and at most 50 replies each. The rest are never read, by this page or by
   any older one. The bound is reported only as a free-text `notice`. The page does notice it
   (`allPageAnswersFor` takes `Boolean(payload.notice)` as "bounded", and the store tracks threads
   whose replies it lacks: `passedOver`, `storeLacksReplies`), but nothing that decides "whole"
   consults that.
2. The page's `has_more` and `next_before` are **Main's**. When the step back reaches Main's first
   message, `has_more` is false.
3. The page sets `discordMoreAbove = false`, and All's cover in the store records `more: false`.
   `historyReadToStart()` is then true, so both the summary and the empty-search sentence claim
   the whole view.
4. Slack's All is never answered from the message cache (`cache/shadow.rs`), so the newer
   served-page cursor is not involved.

**Measured.** Channel A, per Slack's own history: 82 top-level rows (80 real, 2 system events)
and 31 threads with 57 replies, 137 real messages. The server's cache holds the same 137. The
replay (`simulate_flat.py`) gives: page 1 has 50 top-level, 21 threads, replies of 10 loaded (19),
replies of 11 never loaded (23); page 2 has 30 top-level, 10 threads, 15 replies loaded, with
`has_more` false. That is 114 loaded of 137. The owner saw 112; the 2-message difference is not
pinned down (untested candidates: rows hidden as empty or read under Hide read, a reply also sent
to the channel that Flat holds once and the replay counts twice, or the reads being minutes apart).

**Reproduced** in the server's code against the suite's Slack fake: 80 top-level + 62 replies =
142, loaded 120, older page `has_more: false` (evidence 1). That test was not committed: it
asserts today's wrong behaviour. Phase 0 turns it into a test of the fix.

**Also worth knowing.** The agent "going for a long time" posted in channel B until 2026-10-23
(33 top-level, 183 replies) and in channel A since. No search of channel A can reach its earlier
history (question 8).

**The fix (Phase 0)** needs no server change. All is whole only when its cover says
`more: false` **and** no thread root in the store lacks replies (`storeLacksReplies`). Unlike "no
page was bounded", that becomes true once the reader has opened the skipped threads.
`loadedIsWhole`, `historyReadToStart`, `channelSummary` and `searchEmptySentence` follow it, and
the summary always says "loaded". It is not a pure wording change: it changes what the jump logic
treats as the whole view. So it was **not landed** in this pass; see §7.

## 2. What each chat service can do upstream

Full tables are in evidence 2 (Slack, measured) and evidence 3 (Discord and Google Chat,
from their published definitions).

| | Exhaustive read: can prove absence | Text search: hints only | OR | NOT | Phrase | Inside a word | Author by name | Time bounds |
|---|---|---|---|---|---|---|---|---|
| **Slack** (measured, through the private shim's CLI path) | wildcard listing: 10/call, ≤ 200/query, replies included, time-ordered; or history (20/call) + replies per thread | Real-Time Search | no | `-t` | yes | no (prefix) | no | `after:` `before:` `on:` `during:` |
| **Discord** bot | history 100/call (`before`/`after`/`around`) + every thread | Search Guild Messages (bots since 2026-03; `MESSAGE_CONTENT` intent; 202 until indexed; ≤ 10k per window) | no | structural only | via `slop` | no | `author_id` | `min_id`/`max_id` |
| **Google Chat** | list 1000/call with a `create_time` window | `messages:search`, **user auth only** | no | no | yes | no | `sender.name` | `create_time` |

Slack measurements (channel A, evidence 2):

- **Language.** Case-insensitive. `coord` finds "coordinator" (word prefix); `andoff` finds
  nothing (no infix). `"hand off"` works. `review OR rebase` returns the same 5 as `review AND
  rebase` (no OR); `(a OR b)` and `a | b` return 0. `-rebase` and `NOT rebase` work.
  `after:`/`before:`/`on:`/`during:` work. The bridge app's display name finds 1 message where ours
  finds 94: author names are not searched.
- **Not a superset.** Against the page's rule over the same 137 messages, the search missed 4 of
  42 for `review`, 5 of 26 for `landed`, 4 of 44 for `coord`, and 2 of 21 for `agent`. None of the
  misses is an infix match; some are short messages. It also returned many messages ours does not
  match (stemming: 24 extra for `landed`).
- **Exhaustive listing.** `*` for channel A: 14 pages, 22.4 s, 138 messages, every cached message
  present. Channel B: cut at 200 with `source_complete: false` and the reason given.
- **Cost.** About 1.6 s a call, whatever the call. History: 20 top-level rows a call; replies: one
  or more calls per thread; listing: 10 messages a call.

## 3. What vibe-talk has today

- **Page search** (`web/voice.js`, `#129 message-search`, `#262 search-load-older`). Terms split on
  whitespace; a double-quoted run is one term; an unclosed quote runs to the end. A row matches
  when **every** term is a lower-case substring of `data-search`: author + drawn text + every link
  address, over the **combined** text of a glommed row. The search covers loaded rows only.
  "Search before <day>" walks at most 5 pages (15 s, or stops after 3 new matches) per tap.
- **Server cache** (`#208 persistent-cache`, `#384 cache-serve`, `#510`): an SQLite file per
  instance with `message`, `message_scope`, `coverage` (stretches of a scope proven held, provider
  time), `scope_head` (the unsettled band), `thread`, and an **FTS5 trigram index** over
  `search_text` (lower-case author + content + rendered text + link addresses, capped at 8 KB).
  `cache/search.rs` holds the queries "exactly as the search milestone will run them": the index
  drives rare terms, a newest-first `instr` scan handles short or common ones, and every candidate
  is re-matched in Rust with the page's rule. **No route uses them yet.** In production
  `search_text` is already built from the renderer (`render::SearchText`, version `render-1`,
  passed in `cache/lifecycle.rs`; `PlainText` is only the test default). Two parity gaps remain:
  `search_text` always includes the raw `content` as well, so the server matches more than the
  page; and the renderer's `text_content` joins blocks with nothing, where the page's
  `renderedText` puts a line break at block edges and `<br>` and trims. Both candidate paths read
  the capped column, so a term only past 8 KB is never found.
- **Coverage on the private Slack instance (channel A):** Main covered contiguously from the cache's
  age limit to the head, 16 of 31 thread scopes covered, `complete` false everywhere ("reads that
  may have left a message out": the server drops Slack rows it cannot show, such as ones with no
  author). This instance sets `max_days = 14` (the code's default is 90), so nothing older than
  14 days is kept there; the file also has a size ceiling (`max_megabytes`).

## 4. The horizon: "searched back to <date>"

**Definition.** A search's horizon is a claim *H as of A*: **every message of this view that the
server shows, created at or after H and visible upstream at time A, was tested against this
query.** "The whole channel" is the special case where H is the start of the channel's history and
something proved it. The line names who proved each part, and says what it does not cover.

**The trap in All (Slack).** All's reads only bring replies of threads whose root is on the page
read, and at most 10 of those. So a stretch [H, now) of All held by the page or the cache can still
lack (a) replies of passed-over threads rooted inside it and (b) replies posted since H in threads
rooted before H. Neither Main's coverage nor the thread list can say which threads those are. So
on Slack, All's horizon is one of:

- **top-level only:** "top-level messages back to H; replies of N threads not searched", where N
  is the threads the store or cache knows to be incomplete, and (b) is said as a caveat; or
- **whole:** only from a read that lists every message by time, replies included: the wildcard
  listing, recorded as a new "time range" scope in the cache and cross-checked as below.

Main's horizon has no such trap. A bridge's All is its time-ordered whole list, and Discord's
threads are channels with their own history, read and covered one by one.

| Level | Where H comes from | Proven by |
|---|---|---|
| Page (works offline) | the oldest edge of the view's contiguous cover in the page's store (`cover.floor`), or the channel's start when a read proved it and no root lacks replies (Phase 0); on Slack All, top-level only unless every thread held is whole | the reads the page holds |
| Server cache | the oldest `t0` of the coverage chain contiguous from the scope's head, for the view's scope (Main; a bridge's Flat; on Slack All, the new time-range scope); never past `now − max_days` unless the start is proven | the coverage table, which never claims more than a read saw |
| Upstream | each window of an exhaustive read, written into the cache as coverage (`source = 'range'`) as it completes, so an upstream scan *is* a cache backfill, and H moves back window by window | the provider's own listing: Slack's `source_complete` and paging, Discord's history, Google Chat's list; on Slack, cross-checked against history (every top-level ts in the window is present, and each root's `reply_count` matches the replies found) |

**Combining levels.** Each level's claim is a contiguous stretch ending at the newest end it
proved, so where they overlap the union is one stretch and the effective horizon is the **oldest**
of them. Two edges are not proven and are said, not assumed: the unsettled band at the head (about
2 minutes, which coverage never enters; the page's live stream covers it), and a cover saved on the
device that no read has brought up to date. The server searches everything it holds in the
requested stretch; the page drops hits it already holds, by id, so it never has to trust the server
about rows it has.

**Monotone, by construction.**

- The page keeps one claim per (view, query) and replaces it only with an older H.
- The cache's horizon does not depend on the query. A new query starts at once from the cache's
  horizon, and every upstream window read for one search serves every later search.
- New messages at the head arrive through the page's live stream and poll, and are matched locally
  as they arrive. "As of A" therefore stays true without moving H.
- Several things can move the cache's H forward, and each is said rather than hidden: eviction
  past `max_days` or the size ceiling, a coverage interval punctured by an erase, and the oldest
  interval dropped past 512 per scope. A jump to another view starts that view's own claim. Within
  one search the page never shows a newer H than it has shown; if the server's moved forward, the
  line says so ("the server no longer holds …").

**What the page says** (wording to be settled in Phase 2):

> Searched everything loaded here, back to Oct 20 · 3 matches
> Server: back to Oct 11 (its copy) · 2 more further back
> Slack: the whole channel, back to its first message on Sep 30 · nothing more

and, offline: *"Searched what is loaded, back to Oct 24. The server can't be reached to look further."*

## 5. One matcher, two languages

### The language

**Settled (owner, 2026-10-27):** Gmail's syntax as the base plus `from:`, `in:`, `before:`,
`after:` and `has:`, over substring matching that ignores case and accents. Two details differ from
the draft below, and §9 says why: OR binds its two neighbours rather than being the loosest
operator; and `NOT` is a word, because the base's negation is `-`. The
specification is now the module documentation of `vibe-talk/src/search_query.rs`, pinned by
`vibe-talk/tests/fixtures/search_query.json`. The draft follows as it was written.

```
query    := or
or       := and ( "OR" and )*                    OR in capitals; lower-case "or" is a word
and      := unary ( unary )*                      juxtaposition is AND
unary    := ( "-" | "NOT " ) unary | atom
atom     := "(" or ")" | phrase | field | word
phrase   := '"' chars ( '"' | end-of-input )      an unclosed quote runs to the end, as today
field    := ( "from:" | "before:" | "after:" | "on:" | "in:" | "has:" ) value
word     := any run of non-space characters that is none of the above
```

- **Precedence:** NOT, then AND, then OR. A query that is only negations matches everything not
  excluded.
- **A term (word or phrase) matches a message** when its normalized text is a **substring** of the
  message's normalized haystack. That keeps today's core behaviour: `coord` finds "coordinator",
  and `andoff` finds "handoff".
- **Behaviour changes from today**, each to be accepted (question 2 and 10): a phrase now matches
  across a line break (today the drawn text keeps `\n`, so `"hand off"` does not); `-x`, `OR`,
  `NOT`, `(…)` and `from:`/`in:`/`has:`/`before:`/`after:`/`on:` become syntax where today they
  are literal substrings; and the unit is the message, not the combined row (below).
- **Normalization, both sides:** Unicode NFC, then **case folding**, not lower-casing: Rust
  `str::to_lowercase` and JS `toLowerCase` agree, but both map a final Σ to ς by context, so the
  query `ΟΣ` (→ `ος`) misses `ΟΣΑ` (→ `οσα`). The fold is lower-casing followed by a small,
  generated table of the full case-folding differences (ς→σ, ß→ss and the like), one Unicode
  version for both languages, pinned by the corpus. Then every run of whitespace becomes one space.
  Diacritic folding (NFD and dropping combining marks, so `cafe` finds "café") is recommended;
  question 4.
- **Haystack, per message:** author display name + the drawn text of the body + every link
  address, joined by newlines. "Drawn text" is the text content of the HTML the server already
  renders for the page, so the server can compute the same string from its own renderer once
  `text_content` breaks lines where the page's `renderedText` does, and once the raw content is
  dropped from the haystack (or the page adds it: either way, one rule).
- **Fields:** `from:` matches the author's id exactly or the display name as a substring;
  `before:`/`after:`/`on:` are calendar days in the reader's time zone, which the page sends
  (`tz=`), so the server resolves them identically; `in:thread`/`in:main`; `has:link`.
- **The unit is the message, not the row.** Today a glommed row matches when its combined text
  holds every term, even if no single message does. The server cannot see rows, so the proposal is
  that a row shows when any of its messages matches (question 2).

### Single source of truth: WASM, or a corpus? Recommend the corpus.

| | One Rust matcher compiled to WASM | A spec, a JSON conformance corpus, a Rust and a JS matcher |
|---|---|---|
| Matcher parity | by construction | by test: corpus + differential fuzz in CI |
| Haystack parity (the larger risk) | **not solved**: the page's text comes from its DOM | same corpus: *message → haystack* cases, built from server-rendered HTML |
| Page changes | load a `.wasm` asset before the first search, cache it for offline use, a JS shim for strings | ~300 lines of plain JS beside the current matcher |
| Toolchain | `wasm32` target and a bindgen step in the build and in CI; Chromium page tests load it | none new: the repo already runs Rust and Node tests and cross-language differentials |
| Cost | +3–5 agent-days, plus an ongoing build surface | ~1 agent-day beyond the two matchers |

The corpus is `vibe-talk/contract/search/*.json` with three kinds of case: query → parse tree
(errors included), (query, haystack) → match, and message (with rendered HTML) → haystack. Both
test suites load it. A seeded fuzzer generates random queries and texts over a small, adversarial
alphabet (quotes, spaces, Greek sigma, combining marks, `OR`/`or`, `-`) and runs both matchers;
any disagreement fails `make validate` for vibe-talk.

### Mapping a query upstream: a superset prefilter where one exists, hints where not

The server rewrites the query into DNF (OR of ANDs). For each conjunct:

- **Upstream search (hints).** Send only the positive words and phrases. Drop every negation:
  stemming means upstream `-x` can exclude a message our rule keeps. Send OR as separate queries,
  at most 4. Send time fields as the provider's bounds, and `from:` as an id where one is known.
  A conjunct with no term the provider can find (an infix, a term under 3 characters, an author
  name) gets no upstream query. Each hit is fetched in full and re-matched with the exact matcher
  before it is shown. **Hints never move the horizon.**
- **Exhaustive read (proof).** Query-independent: the server reads windows of history into the
  cache and re-runs the cache search over each window. This is the only thing that moves the
  horizon past the cache.
- **Long messages.** Candidates come from the capped `search_text`; a row whose text was capped
  (`was_capped`) is always re-matched on its full content, by a scan of the capped rows in the
  stretch, so the horizon's "every message was tested" holds for long posts too.

## 6. Sparse catch-up: just the hits

**API** (one new route; the existing ones are unchanged):

```
GET /api/v1/channels/{id}/search?q=&tz=&view=main|flat|thread&thread=&below=<instant>&threads=<ids>&limit=50&cursor=&upstream=none|scan|hints
→ { hits: [Message, newest first],       // older than `below`, plus any hit in the `threads` the page lacks replies of
    next: cursor | null,                 // more hits in the searched stretch
    horizon: { from_ms, start: bool, as_of_ms, by: "cache" | "upstream" },
    upstream: { state: "idle" | "running" | "done" | "failed", windows_done, eta_s } | null,
    hints: [Message] }                   // upstream-search hits, checked by our matcher, outside the horizon
```

`upstream=scan` starts (or joins) a bounded backfill job for the channel. The page polls the same
route with the cursor; each answer carries the new horizon and any new hits. The bounds are like
today's walk: a time budget per tap, and Stop. `below` alone would hide replies the page never
read inside its own stretch (passed-over threads, and late replies to older threads), so the page
also names the threads it lacks replies for, and the server searches those whatever their age.
The server matches with FTS5 trigram candidates for positive terms of 3 or more characters and a
newest-first scan otherwise, re-matches every candidate with the shared matcher on the full text,
and re-scans rows whose `search_text` was capped (§5), so the 8 KB cap costs no recall.

**The page.**

- Hits older than the loaded stretch go into the channel's store as *islands*, with a flag.
  Today the store counts every reply it holds (`replyTally`: held, and unread unless dealt with)
  and `projectView` hides rows below a floor only while the cover says `more`. So islands must be
  kept out of the tally, out of `storeLacksReplies`, and out of the projection explicitly, and it
  must be decided whether they are saved on the device (proposed: not; they are search results).
- They are drawn in a "Further back" list above the step-back control: grouped by day, newest
  first, the same row renderer, the same search marks, and a "Show in channel" action. That action
  jumps with an `around` read (Discord `around`, Slack `latest`/`oldest` around the ts), not a walk
  through every page in between.
- Read state is per message id, as today: Done on an island marks it; with the flag above, an
  island arriving changes no unread count or "N new" chip.
- The search count says both parts: "3 here · 2 further back".
- Offline: the page searches what it holds and says the server could not be asked. Nothing is
  shown as searched that was not.

## 7. Phased plan

| Phase | What | Cost (agent-days) |
|---|---|---|
| **0. Honest counts** | Page only: All is "whole" only when its cover says `more: false` and no root in the store lacks replies (`storeLacksReplies`); `loadedIsWhole`, `historyReadToStart`, `channelSummary` and `searchEmptySentence` follow; the summary always says "N loaded" and names a total only when proven ("112 loaded in All · replies of 11 threads not loaded: open them, or use Main"). A page test for the sentence and the summary over a bounded All; the reproduction above becomes a server test that the bound is still reported. | 0.5–1 |
| **0b. All without the bound** (optional) | Read Slack's All's **older** pages via the wildcard listing: time-ordered, replies included, 10 a call; the newest page stays on history, since how soon search indexes a new message is unmeasured. The shim gains one method; Flat's backward cursor becomes a time bound. Removes the 10-thread bound and most per-thread calls. | 2–3 |
| **1. One language** | The spec, a Rust parser/matcher, a JS parser/matcher, the corpus and the differential fuzz in the validation DAG; the page moves to the new matcher (message as the unit); the server's `search_text` made to equal the page's haystack: `text_content` breaking lines as `renderedText` does, raw content in or out on both sides (a new `SearchTextSource` version, rebuilt in the background); the fold table generated once for both languages. | 2–3 |
| **2. Search the server's copy** | The route above with `upstream=none`: cache search below the page's floor, horizon from coverage, the "Further back" list, the horizon line, offline wording. | 2–3 |
| **3. Search upstream, exhaustively** | The backfill job: Slack wildcard windows (split at 200, cross-checked against history), Discord history plus threads, Google Chat list windows; coverage written as `range`; on Slack a new time-range scope (the `Scope` enum, `message_scope`, serve rules, race tests); budget, consent, Stop, a per-channel cost estimate shown before the first tap; the horizon reaching "the whole channel". | 4–6 |
| **4. Upstream hints** (optional) | Slack Real-Time Search, Discord Search Guild Messages, Google Chat `messages:search` (where a user token exists): DNF split, full fetch, exact re-match, shown apart from the horizon. | 1–2 |

Phases 0 and 1 are independent; 2 needs 1; 3 needs 2; 4 needs 2. Total about 10–15 agent-days,
plus 2–3 for 0b.

**Risks.**

- Slack's rate limits are shared by everyone using the same app, so backfill must be paced and
  stop on the first rate limit.
- The wildcard listing was seen complete on one young channel only; file-only messages and
  indexing delay are unproven, hence the cross-check against history and history at the newest end.
- This instance's `max_days` (14) bounds its cache: without question 6, every search past 14 days
  re-reads upstream.
- Changing the match unit from row to message changes what a few searches find today.

## 8. Questions for the owner

1. **Phase 0 first?** Land the honest counts now (under a day), and decide separately on 0b (All
   without the 10-thread bound, 2–3 days)? *Recommended: yes to 0 now, 0b after Phase 2.*
2. **Match unit:** may a combined row show only when one of its messages matches all the terms,
   rather than the row's combined text? *Recommended: yes; the server has no rows.*
3. **Substring, not words:** keep today's substring matching (`coord` finds "coordinator", an
   infix finds the word that holds it)? The cost is that upstream search can never be an exact
   prefilter, only hints. *Recommended: keep substring.* **Answered 2026-10-27: yes, keep substring**, even where it clashes with upstream word search; upstream is a candidate source only.
4. **Accents:** should `cafe` find "café" (diacritic folding on both sides)? *Recommended: yes.* **Answered 2026-10-27: yes**, case- and accent-insensitive by default.
5. **Consent and cost upstream:** start an exhaustive upstream scan only on a tap that shows its
   estimate ("Search Slack further back: about 80 s for this channel"), bounded like today's walk,
   or in the background on every search? *Recommended: on a tap.*
6. **Keeping history past 14 days:** the Slack instance keeps 14 days (`max_days = 14`; the code's
   default is 90). Raise it, add a search-only index (ids, times, `search_text`) kept longer, or
   re-read upstream every time? A privacy trade-off. *Recommended: raise `max_days` to the default
   90 first; a separate index only if that is not enough.*
7. **Upstream hints:** do you want fast far-back hits from the services' own search (Phase 4), shown
   apart because they can never prove absence? Google Chat's needs a user token we do not hold.
   *Recommended: Slack and Discord only, after Phase 3.*
8. **Across channels:** channel A's agent has history in channel B. Search several registered
   channels at once (an alias group), or one channel at a time? *Recommended: one at a time now;
   a "related channels" group later.*
9. **Context with far-back hits:** show each hit alone with "Show in channel", or with one or two
   neighbours? *Recommended: alone, plus "Show in channel".*
10. **Syntax:** OR in capitals only (lower-case `or` stays a word), `-term` and `NOT term`,
    parentheses, and `from:` `before:` `after:` `on:` `in:` `has:`. Anything to add or drop?
    **Answered 2026-10-27:** Gmail syntax as the base plus the shared fields; see §9.

## 9. Precedents: how five products spell a search

`#596 server-search-prep`, 2026-10-27. The owner asked for the query syntax of Google web search,
Gmail, Slack, Discord and Google Chat, checked against each product's public help pages, and for
ours to be a compatible superset with Gmail's as the base. The pages were fetched read-only on
2026-10-27:

- Google web search: [Refine Google searches](https://support.google.com/websearch/answer/2466433),
  the [Advanced Search form](https://www.google.com/advanced_search), and
  [search tips](https://support.google.com/websearch/answer/134479).
- Gmail: [Refine searches in Gmail](https://support.google.com/mail/answer/7190).
- Slack: [Search in Slack](https://slack.com/help/articles/202528808-Search-in-Slack), plus what
  evidence 2 measured.
- Discord: [How to Use Search on Discord](https://support.discord.com/hc/en-us/articles/115000468588).
  The article was read through the help centre's JSON API, because the page itself answers with a
  browser challenge.
- Google Chat:
  [Use search operators in Google Chat](https://support.google.com/chat/answer/14179758).

"Not documented" means the cited page does not say. It does not mean the product lacks the feature.

| | Google web | Gmail | Slack | Discord | Google Chat | **Ours (settled)** |
|---|---|---|---|---|---|---|
| Several words | all of them, but "might include results that only contain some of the terms" | all | all; measured through the API: not strict (`gate report` returned messages lacking one word) | not documented | all (API: implicit AND) | **all** |
| `AND` keyword | not documented | `from:amy AND to:david` | measured: works | not documented | not documented | **`AND`, capitals only** |
| OR | `miniature OR standard` (Advanced Search) | `OR` or `{ }` | measured: none (`OR` read as AND) | not documented | not documented (API: none between keywords) | **`OR`, capitals only; binds its two neighbours** |
| `\|` | not documented | not documented | measured: matches nothing | not documented | not documented | a word |
| Exclude | `-car`, `-"Jack Russell"` | `dinner -movie` | `marketing -report`, `-in:`, `-from:` | "author type" filter only | `-birthday`, `-from:jenny`, `-has:file` | **`-term`, `-"phrase"`, `-field:`, `-(group)`** |
| `NOT` | not documented | not documented | measured: works | not documented | not documented | a word |
| Exact phrase | `"tallest building"` | `"dinner and movie tonight"` | `"marketing report"` | not documented | `"happy birthday"` | **`"…"`; an unclosed quote runs to the end** |
| Grouping | not documented | `subject:(dinner movie)` | not documented | not documented | `in:(mobile phones)` | **`( … )`** |
| Author | not documented | `from:amy@example.com`, `from:me` | `from:@Sara`; via the API only `from:<@U…>` (evidence 2) | `from:` | `from:john@xyz.com`, `from:me` | **`from:` an id exactly, or part of the display name** |
| Where | `site:` | `in:anywhere`, `in:archive`, `label:` | `in:#channel`, `in:@person`, `is:thread`, `with:` | `in:` a channel | `in:<space>`, `is:dm`, `is:room` | **`in:thread`, `in:main`** (within the channel being searched) |
| Dates | `before:2025/11/01`, `after:2026/03/10`, `after:2019-03-01` | `after:2004/04/16`, `before:04/18/2004`, `older_than:1y`, `newer_than:2d` | `before:`, `after:` (measured: excludes its day), `on:`, `during:august` | on, before or after a date (filter) | `after:2023/04/16`, `before:04/18/2023`, `older_than:`, `newer_than:`, `tz:` | **`before:` and `after:`; `YYYY-MM-DD`, `YYYY/MM/DD` or `MM/DD/YYYY`, in the reader's zone** |
| Contains | `filetype:` | `has:attachment`, `has:drive`, `filename:` | `has::emoji:`, `has:pin`, `is:saved` | `has:` links, embeds, files… | `has:url`, `has:file`, `has:image`… | **`has:link`** |
| Mentions | not applicable | `to:`, `cc:` | not documented (`with:` for threads and DMs) | `mentions:` | `at:` | not yet |
| Status | not applicable | `is:unread`, `is:starred` | `is:saved` | `pinned` (API) | `is:unread` | not yet |
| Wildcard | not documented on these pages | not documented | `rep*`: words beginning with at least three letters; measured through the API: no effect (`gat*` 0 hits, `coord*` = `coord`) | not documented | not documented | none needed: every term already matches inside words |
| Case | "A search for New York Times is the same as a search for new york times" | not documented | measured: insensitive | not documented | not documented | **insensitive (full case folding)** |
| Accents | not documented | not documented | not documented | not documented | not documented | **insensitive (combining diacritics dropped, `ø`→`o`, `ł`→`l`…)** |
| Unit matched | words and related words ("running" for "jogging") | words; "a space or a dash (-) will separate a number" | measured: whole words and their inflections (`landed` finds "land"); not a longer word's start (`coord` missed 119 "coordinator…"), never inside a word | API: tokens with `slop` | API: keywords | **substrings of the message's folded text** |

**Ours is Gmail's syntax plus the fields the chat services share.** The base is Gmail's: implicit
AND, an optional `AND`, upper-case `OR`, `-` before a term, phrase, field or group, `"phrase"` and
`( … )`. The fields are the ones at least three of the chat services have and that make sense
inside one channel: `from:`, `in:`, `before:`, `after:` and `has:`. A field with a value we do not
know (`has:file`, `in:#general`, `on:2026-10-24`, `during:october`) is read as an ordinary word,
just as Gmail reads an operator it does not know. So a query written for Slack, Gmail or Google
Chat searches something sensible here and never fails to parse. Where the products agree, we agree:
`-from:`, quoted phrases, the `YYYY/MM/DD` and `MM/DD/YYYY` dates that Gmail and Google Chat accept,
and Google's ISO dates.

**Where we deliberately differ:**

- **Substring, not words** (owner, 2026-10-27). `coord` finds "coordinator", `andoff` finds
  "handoff", and `f(x)` finds "call f(x)". Every product above matches words or word prefixes.
  This is why upstream search can only ever offer candidates (§5, and the probe in
  `ai_docs/transient/2026-10-27-slack-search-probe/`).
- **Case- and accent-insensitive by one written fold** (owner, 2026-10-27). The fold is:
  - NFD, with the combining diacritic blocks dropped, then NFC;
  - lower-casing;
  - a table that completes Unicode's full case folding (`ς`→`σ`, `ß`→`ss`, `µ`→`μ`, ligatures)
    and joins letters whose mark is built in (`ø`→`o`, `ł`→`l`).

  Only the diacritic blocks are dropped. Indic and Thai vowel signs, Hebrew and Arabic points and
  Japanese voiced marks are combining marks too, and dropping them would make `कि` find every
  `क`. The corpus pins both.
- **OR binds its two neighbours**: `a b OR c` is `a AND (b OR c)`. None of the help pages cited
  above states a precedence. A draft of §5 had OR as the loosest operator instead. Parentheses
  settle either reading.
- **`OR`, `AND` and `NOT`.** Only the capitalised `OR` and `AND` are operators, so `or` stays a
  word. `NOT` is a word as well, because the approved base is Gmail's `-`. A product that reads
  `NOT` (Slack does, by measurement) loses nothing, since `-` does the same job.
- **`after:` includes its day and `before:` excludes its own**: `after:D before:E` is the days
  D up to but not including E. Gmail's page does not say which way it reads `after:`. Slack's
  `after:` was measured to exclude its day (the probe).
- **`in:` names a part of the channel, not another channel.** Searching several channels is open
  question 8.
- **No `{ }`, `|`, `AROUND`, `*`, `on:`, `during:`, `older_than:` or `newer_than:` yet.** Each is
  simple to add once the owner wants it. Until then each is a word, so adding one later changes
  only queries that already contain it.

## 10. Existing crates: index, query parser, matcher

The owner asked (2026-10-27) to check existing crates before building the query DSL, the matcher
or the index. These options were evaluated in a scratch benchmark, built outside the repository; its source is
`ai_docs/transient/2026-10-27-slack-search-probe/scripts/index_bench.rs`:

- (a) SQLite FTS5 in the store vibe-talk already uses;
- (b) tantivy;
- (c) query-parser-only crates.

The corpus was this harness's busiest Slack channel: 216 real messages, median 656 characters,
lower-cased. They were replicated to 20,000 and 110,000 rows; the second is the size the store's own
benchmark uses. Each row had four random words from the corpus vocabulary and a unique token
appended. "Incremental" means 1,000 more rows in commits of 50. The live volume, as `#595
cache-full-history` measured it with `vibe-talk cache-stats` on a copy of the live store: 2,300
messages in 19.3 MB, 8,378 bytes a message, with the trigram index 52% of the file. That is a
fraction of either benchmark size.

| | FTS5 trigram, `detail=full` (today's schema) | FTS5 trigram, `detail=none` | tantivy 0.27, 3-gram + lower-case + ASCII folding | plain table, no index |
|---|---|---|---|---|
| File at 110k rows (93 MB of text) | 390 MB (index ≈ 276 MB, 3× the text) | not run at 110k | text in SQLite 113 MB + index 64 MB | 113 MB |
| File at 20k rows (17 MB of text) | 72 MB | **32 MB** (index ≈ 12 MB) | 20.5 MB + 13 MB | 20.5 MB |
| Bulk build, 110k | 46–51 s | (20k: 5.0 s against full's 6.4 s) | 1.8 s | 0.6 s |
| Incremental, 1,000 rows in commits of 50 | 0.3–0.9 s | 0.25 s (20k) | 1.5–2.4 s | ~0 |
| `gate` at 110k (7,333 true matches) | 1.7 ms, exact | (20k: 3.4 ms, exact, via `LIKE`) | 1.0 ms, **9,433: not exact** | 147 ms scan |
| A unique token at 110k | 0.3 ms | (20k: 0.3 ms) | 0.1 ms | 167 ms scan |
| Substring | exact for 3+ characters; shorter terms scan | the same, through `LIKE '%…%'` (phrase `MATCH` refuses `detail=none`) | approximate: a phrase of 3-grams gave 29% false positives for `gate` | exact, by scan |
| Case and accents (probed) | case folded (`ΟΔΟΣ`=`οδος`); `remove_diacritics 1` or `2` folds `café`, `naïve`, `İstanbul` but **not** `Łódź`→`lodz`, `Straße`→`strasse`, `ﬁnal`→`final` | the same | ASCII folding (Latin only) | none |
| Consistency with the store | same transaction, triggers, `secure-delete` erasure | the same | a second store: its own commits, crash recovery, and deletes that are tombstones until a merge | — |
| Licence | public domain (bundled 3.53.2) | the same | MIT | — |
| Dependencies | none new | none new | **107 crates, 76 not already in vibe-talk's tree; +9.5 MB to a release binary** (2.6 → 12.1 MB in the bench) | — |

**Query parsers.**

- `search-query-parser` 0.1.4 (MIT; eyre, regex, serde) parses `and`/`or` in any case, binds AND
  tighter than OR, and has no fields. Each of those three contradicts the settled syntax.
- tantivy's `QueryParser` is field-and-term syntax for its own index. It has no substring
  semantics and no JavaScript twin.
- A pest or nom grammar would cover Rust only, and the page needs an identical parser in
  JavaScript.

Whichever parser the server uses, the parity risk sits in a second hand-written parser on the page.
The corpus and the differential fuzz exist to control exactly that.

**Recommendation: keep FTS5's trigram index, hand-roll the parser and matcher, and fold before
indexing.**

1. **Matcher and parser: hand-rolled, done.** They are `src/search_query.rs` (about 600 lines
   of code and specification besides its tests) and its twin in `web/voice.js` (about 290 lines
   of code). Both are held to `tests/fixtures/search_query.json` (31 fold, 76 parse, 55 match
   and 22 upstream cases). A seeded differential runs the page's code under node against the
   Rust code on 4,000 random queries, texts, authors and links. The Rust suite also reads the
   page's fold table, white-space set and diacritic blocks back from `voice.js` and compares
   them both ways. No crate fits the settled syntax, and none helps the page.
2. **Index: FTS5 trigram, as today, over a folded column.** No engine's own folding equals the
   page's, so the stored `search_text` should hold `fold(…)` of the haystack, under a new
   `SearchTextSource` version that the writer rebuilds in the background. Query terms get the same
   fold. Trigram matching is then exact substring matching for terms of 3 or more characters.
   Shorter terms, or terms containing `%` or `_`, take the scan. The Rust re-match stays as the
   final word. Not done in this pass: it rewrites every row of the live cache on deploy, which the
   coordinator should schedule.
3. **Size: move the index to `detail=none` and query it with `LIKE`.** At 20k rows that cut the
   index from 51 MB to 12 MB, about tantivy's size, and the counts stayed exact. Today's index is
   about 3× the text. At the live store's 8.4 KB a message, the default `max_megabytes = 256`
   holds about 30k messages. `#595 cache-full-history`'s `backfill = "full"`, which keeps whole
   histories up to 16 GB, makes the index's size matter more. It is a schema migration (drop and
   rebuild the virtual table), so it was not done here.
4. **Revisit tantivy** only if the folded `detail=none` index proves too large or too slow on the
   live volume. It would add a second store to keep consistent with erasure and eviction, a 4.6×
   larger binary, and 76 new crates.

## 11. Groundwork landed, and what waits on a question

`#596 server-search-prep`, 2026-10-27. Every item is either unreachable from the page or off by
default. Nothing below changes what the page shows today.

**Settled by the owner on 2026-10-27, and now written into the code and the corpus:**

- question 3, substring matching;
- question 4, accents;
- question 10, the syntax, as summarised in §9.

**Done (question-independent):**

- **One language, two copies.** `src/search_query.rs` (the specification, in its module
  documentation) and its twin in `web/voice.js` (not yet called by the page) share the conformance
  corpus `tests/fixtures/search_query.json` and a seeded cross-language differential, run by
  `cargo test`. The page suite runs the corpus too, with negative controls.
- **The upstream candidate plan** (`upstream_plan`, `searchUpstreamPlan`) follows the owner's
  2026-10-27 answer on substring matching:
  - negations and fields are dropped, since dropping a condition only widens the candidate set;
  - the rest goes into disjunctive normal form, at most 4 upstream queries;
  - each alternative sends only its word-aligned terms (3+ letters or digits);
  - an alternative with none reports why the cache must be scanned for it: `not-word-aligned`,
    `no-term` or `too-many-alternatives`, so that the status can say so.
- **Horizon bookkeeping.** `coverage::horizon` follows the claim chain down from the head's settled
  top to the first gap, clamped at the age limit. A property test checks it against `covers`.
  Not yet used: `#595 cache-full-history`'s record of a channel walked to its beginning, which is
  what lets a horizon say "the whole channel".
- **A cache search** (`MessageCache::search`):
  - it scans newest first, below the page's floor, within a row budget;
  - it matches every row with the shared language;
  - it returns the hits, where to resume, how many rows had a capped or partial text, and the
    view's horizon, read in the same snapshot;
  - on Slack's All the horizon covers top-level messages only.

  No route reaches it.

**Not done, and why:**

- **The `/search` route.** Serving cache rows to a caller has rules (`Sink::try_serve`): the
  account the rows were read with, a suspended provider, a channel in doubt. A route that bypassed
  them could serve one account's rows to another. Building that path is Phase 2 proper. It is
  question-independent but not small.
- **The folded `search_text` and the `detail=none` index.** These are the §10 recommendations
  2 and 3. Both rewrite the live cache file on deploy.
- **The page moving to the new matcher.** That waits on question 2 (match a combined row only
  when one of its messages matches).
- **An upstream candidate provider for Slack.** Classic `search.messages` cannot be reached on our
  path. Real-Time Search could stand in, but it finds whole words only: 72% recall of our
  substring matches over 20 terms. The shim has no method for it yet (the shim is private,
  outside this repository), and whether to show upstream hints at all is question 7.
- **The server's haystack and `has:link`.** `MessageCache::search` matches on the stored
  `search_text`, which also holds the raw content (the §3 parity gap), and it approximates
  `has:link` as `://` anywhere in it. Both close when the column is rebuilt from the page's
  haystack.

**Skipped because they depend on an open question:**

| Step | Waits on |
|---|---|
| Page's search box on the new matcher; row vs message unit | 2 |
| Upstream scan job (consent, budget, cost estimate shown first) | 5 |
| Retention or a longer-lived search index | 6 |
| Upstream hints shown apart from the horizon | 7 |
| Several channels at once (`in:` naming channels) | 8 |
| Context around far-back hits | 9 |
| Phase 0's page wording | 1 |

## 12. The build (2026-10-27, `#603`-`#608`)

The owner answered every question in §8 on 2026-10-27 (Q1 yes; Q2 view rules below; Q3, Q4, Q10 as
in §11; Q5-Q7 a button that prioritises our own crawl, full history instead of 14 days, upstream
hints deferred; Q8 an "All channels" checkbox; Q9 hits alone with Show in channel). The epic is
`#521 server-side-search`; each phase has its own issue.

- **`#603` Phase 0.** Slack's All reads the replies of every thread on its page (the ten-thread bound
  is gone). A thread of at most 50 replies that the message cache holds whole as its root describes
  it (same count, same latest reply, coverage from the root, the band vouched, listings within 15
  minutes and after any dropped offer) comes from the cache at no call, for a write-scope caller with
  serving on; replies taken so are kept out of the fill. Landed as 6c9d067a.
- **`#605` the route.** `GET /api/v1/channels/{id}/search` and `GET /api/v1/search` search the cache
  under the serving rules (`Sink::search`: cache open, serving on, channel routed, account verified,
  not suspended, scope not blocked; a reply of a blocked thread is withheld), and answer with each
  channel's scope: how far back the search reached, what the cache vouches it holds, whether the
  channel is whole or still expanding, and where to resume. Upstream is always `none`.
- **`#606` the view rules.** The page matches messages with the shared language: Main keeps a root
  when any held reply matches, All matches each message, the cards choose a thread with any hit and
  an unpacked thread shows only its hits with folded lines between them.
- **`#607`** hits older than the loaded stretch drawn alone with Show in channel; All channels.
- **`#608`** Search further back calls `#602 history-settings-and-limits`' prioritize route and polls
  its state, searching again as the copy grows; Stop takes the bump back.
- **`#604` the index** (folded `search_text`, LIKE over the `detail=none` trigram index `#602` landed)
  follows separately, because it rebuilds the live cache's index text on deploy.

## Evidence

- `evidence/01-flat-bound.md`: channel A's real shape, the replay, the reproduction test and its
  output, and the chain of code.
- `evidence/02-slack-search-pilot.md`: Slack's query language, the hit-set comparison, the
  wildcard listing, and costs.
- `evidence/03-discord-google-chat.md`: Discord and Google Chat, quoted from their published API
  definitions, with source commits.
- `evidence/scripts/`: `walk_history.py`, `search_pages.py`, `compare_sets.py`, `simulate_flat.py`.
  They read only; the vendor CLI is named by the caller through environment variables.

What was not verified: Discord and Google Chat were not called live (no tokens were used); how
soon Slack indexes a new message; whether Slack's wildcard listing includes file-only messages, or
is complete beyond one young channel; whether a bot token can make the same calls; the 2-message
gap between the replay (114) and what the owner saw (112).

## Review

An independent reviewer checked this plan against the code before it landed. It confirmed the
Flat mechanics, the replay and reproduction numbers, that Slack's All is never served from the
cache, the description of the page's matcher, and that dropping negations upstream is safe. Its
corrections are folded in above:

- All's horizon on Slack (passed-over threads and late replies to older threads);
- `search_text` already coming from the renderer, and the real parity gaps;
- the 8 KB cap;
- phrases across line breaks and the other behaviour changes;
- case folding versus lower-casing;
- islands versus the reply tally;
- what else moves a cache horizon forward;
- Phase 0 needing no server change;
- the wildcard listing's evidence;
- `max_days`' default;
- redactions.
