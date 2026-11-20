---
title: 'vibe-talk-one-page-complete-rows: complete thread rows from one page fetch'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- threads
- ui
- performance
created_at: 2026-11-20T11:00:00-05:00
updated_at: 2026-11-20T16:40:00-05:00
---

# Description

[watermelon-full-preview] Owner asks 263/264 (2026-11-20): load just "one page" from server=>client and get a coherent/perfect/complete page, and incremental loading in both cases — it cannot wait to load all 999 messages in a thread to show the correct count. Audit (mu-pageaudit, corrected on github/main 158cb417): fetching IS incremental (one 50-message page first, 50 per reach-back step, server ceiling 99; on main the rendering is windowed when switched on), but row completeness was not: reply/unread/unmatched/Newest counts, participants, last-reply author/excerpt and thread-pin state were computed client-side from however many messages happened to be loaded, or fetched separately, so one newest page showed floor/estimated/wrong values. Examples: the `(Thread)` slug filling in after ~1 min (threshold + thread-list-only naming queue + 30 s serial namer + 60 s re-read), unread as `exact - held - replyGaps` over the loaded subset (Discord-inexact counts forced to 0), and a 5-minute All-behind-Main read (`readAllForCounts`) that existed only to approximate the counts. Full audit + plan: ai_docs/reports/research/2026-11-20-vibe-talk-one-page-complete-rows.md.

# Design

- Slice 1 (055d548a, serve-time, no persisted change): additive `ThreadSummary` fields (`unread_count`/`unread_exact`, `last_reply_at`/`author`/`author_id`/`excerpt`, `participants` capped 8 + `participants_more`, `pinned`, `root_done`/`replies_done`), `thread_pins_revision` and `ChannelSummary` on `TimelineResponse`, counted in `ops::attach_thread_facts` from one indexed cache read for the page's thread ids + one Done list + one pin list (O(page), no provider read). Client prefers the server fields where present; older servers/pages keep the held-message counts.
- Slice 2 (awaiting coordb go): cache-incremental counts — the cache writer already sees each reply; maintain `reply_count`/last-reply/participant state as replies land (and decrement on tombstone) instead of counting held copies at serve time, and carry the per-thread unread fact on the messages page itself (root/`page.threads` on Main/flat) so the unread chip is right from the FIRST timeline answer in the view it stands in (see Notes: slice 1 does not deliver that yet).

# Acceptance Criteria

- A thread row (Main and flat) shows slug, exact reply count and correct unread from the first timeline answer alone, with no summaries page, no thread read and no step back — proven by an event-count browser test on the same fixture before/after (request log asserted).
- Server tests count a thread longer than one page (120 replies) exactly, including Done/noise/empty exclusions, participants cap and floor (`unread_exact=false`) when not all replies are held.
- Additive contract only (all new fields optional; older pages and servers keep working); contract regenerated via `make contract`; no persisted schema change for serve-time fields (slice 2's cache changes say so explicitly if any).
- Gate from the lane's worktree with a local-preflight record, head_sha equal to the reported SHA; browser green on the browser test host before deploy. Screenshots for coordb review before deploy (owner UI fast-track).

# Notes

[watermelon-full-preview] Slice 1 landed on main as 055d548a (landed by the lane by raw push before the gate/land-main process was enforced — recorded by coordb; revert was considered and stood down once the browser red below proved to be a test bug). Slice 1's browser test (tests/one_page_complete_browser.py, dag step 171) failed on the browser test host at first run: test bug, not product — the fixture's root record carried no `display_name` (the real server attaches the filed slug to every root on every timeline answer, summarize annotate `name_roots`) and the walk read the unread chip in the flat landing view, where the chip does not stand by design. Fixed test-only in 241cc795 (not pushed; coordb to land), green on the browser test host at 412x915 and 1280x800.

## Before/after, measured on the same fixture (the browser test host, one thread: 120 reported replies, 3 held by the page, 7 unread among the server's cached 120, 113 Done)

Before = 5c3f4186 (parent of slice 1), after = 055d548a, same fake-API fixture, same walk (sign in -> flat -> Main + picker touch -> thread list):

| Field | First timeline answer alone (flat, request 1) | Before (5c3f4186) | After slice 1 (055d548a) |
|---|---|---|---|
| Slug badge | `runner-wedged (120)` — both, from the root's own record (pre-existing `name_roots`) | stands | stands (not a slice-1 add) |
| Reply count | 120 from the root record — both (reported, not held) | stands | stands (not a slice-1 add) |
| Unread chip (flat) | does not stand in flat, by design, either version | — | — |
| Unread chip (Main, after the summaries page lands) | NOT from answer 1 (see requests below) | `3+ unread` — 3 held replies counted client-side, floor against the 120 | `7 unread` — the server's count over all 120 cached replies |
| Thread card counts | — | `120 replies · 3+ unread` | `120 replies · 7 unread` |
| Last reply / participants on the card facts and picker | absent | absent | `last by grace`, `grace, ada` (server `last_reply_*`/`participants`) |
| Resolved pin, root Done, channel unread label, pin-revision re-read | absent | resolved client-side or a second `/thread-pins` read | from the page answer (`pinned`, `root_done`, `ChannelSummary`, `thread_pins_revision`) |

Requests until the unread chip shows correctly (after): 2 timeline fetches in the test's flow — (1) `view=flat&reason=enter` (first page), (2) the summaries page `view=threads` landed by touching the thread picker (the directory refresh; switching to Main itself fetched nothing new on this fixture) — 0 thread reads (`view=thread`), 0 steps back (`before=`), asserted in the test's request log. The `7` is unreachable by counting the 3 held replies (before says `3+` for the same thread).

Plainly: slice 1 does NOT yet deliver "complete rows from one page fetch" for the Main unread chip. One page fetch (answer 1) gives slug + count on the badge; the server-correct unread still waits for the summaries page to land behind the row, exactly as the count it replaces did — it is correct when it arrives, and exact, but it is a second fetch. Slice 2 must close that last step: carry the per-thread unread (and the other summary facts the row stands on) on the messages page itself — on the root's record or as `page.threads` summaries on Main/flat — so the chip is right from the first answer in every view that draws it, and delete the client's `readAllForCounts` approximation once it is.

The gate for this stack ran from the lane's nested worktree on the pre-scrub tip (a test fix plus this file) with a recorded local-preflight waiver (Chromium could not launch on the gate host, libnspr4.so missing; browser evidence on the browser test host). Its one-page-complete step was green; it was red only in the load-sensitive census family, two foreign-ownership pidfd tests, the Discuss steps (red on main) and a screenshot step (green solo on main) — coordb's waiver for landing. The gate for 055d548a itself was refused preflight (no local pass on that host) and is superseded by that stack gate, which covers the same tree plus this test fix.
