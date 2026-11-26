---
title: 'vibe-talk-state-sync-audit: no page request ever blocks on a provider read; every exception fixed or justified, with a gating test'
status: open
priority: 0
issue_type: bug
assignee: mu-statesync
labels:
- vibe-talk
- cache
created_at: 2026-11-26T03:45:00-05:00
updated_at: 2026-11-26T10:27:32-05:00
---

# Description

Owner request 381 (2026-11-26 3:35 AM ET), verbatim: "Wait what. Why would slacks rate limit matter!!!!! The client is not supposed to go to slack only to the vibe talk server. Do we have any violations of that core architectural state sync principle!!!? We need to focus on that as well as perf and optimization lanes doing both benchmarking profiling and analyzing event dependencies and data and control flow inside the client. Are we completely done w our earlier typescript modularity refactor?"

The principle: the page talks only to the vibe-talk server, and the server answers page requests from its own state; upstream provider reads happen in server-scheduled background work, never on a page request's critical path.

On the night that prompted the request, every page request was answered by the server within 106 ms and none waited on the provider; the rate limit hit a server background refresh, so the server's copy lagged for 26 threads. At 2026-11-26 3:27 AM ET an All read gave up on 25 threads' replies when the server's own budget refused (see Notes on coordination with the server-side fix).

This issue audits whether the principle holds everywhere in code, not only in the path observed that night. Any path where a page request can block on, start, or wait for a provider read — directly or indirectly — is a violation to fix or to justify explicitly.

# Design

1. Enumerate every path where a page request can block on or start a provider read, including: the documented cache-first exceptions (a cold view served provider-first, place and reply-place reads, the counts and older paths, fallback timers), the first open of a channel or thread, search, catch-up and summarize inputs, glossary and explain lookups, document fetches, read-state writes, and anything else found. For each path record the file and function, whether it blocks the answer or only enqueues background work, and a verdict: FIX (make it server-scheduled with a live event) or JUSTIFY (why it must stay, with its worst-case latency).
2. Add a gating test that fails if a page-facing route awaits an upstream call: a fake provider that records calls, driven through the real server router with the page's exact requests, asserting zero awaited upstream calls per page route. The test waits on events or conditions only, uses no wall-clock budgets, and holds at hot load.
3. Land the FIX verdicts as small gated steps, each with tests and a dynamic exercise through the real server before landing. JUSTIFY verdicts keep their latency bound in a test or in code, so the exception cannot grow silently.
4. Record whether the TypeScript modularity refactor is complete (`#624 voice-page-modularity`, closed 2026-11-15), with evidence from the tree, since the owner asked in request 381.
5. Deliver the survey table in a harness-repo report before any fix is gated; fix steps follow from its verdicts.

# Acceptance Criteria

- The survey table covers every page-facing route and every path listed in Design item 1, each with file, function, blocks-or-enqueues, and a FIX or JUSTIFY verdict; no page request awaits a provider read except a justified exception with a stated worst-case latency.
- A gating test through the real server router, with a call-recording fake provider, asserts zero awaited upstream calls for each page route outside the justified exceptions; it is event- or condition-based, has no wall-clock budget, is classified in the validation manifest, and passes at hot load.
- Every FIX verdict lands as a small gated step with tests, and the changed behaviour is exercised end-to-end through the real server (stub only external upstreams) before landing, with evidence.
- The background-refresh rate-limit gap is closed without double-fixing: the All-read budget refusal of 2026-11-26 3:27 AM ET is owned by the server-side fix in `#2278`; this issue coordinates with it and does not change the same code twice.
- The report answers, with tree evidence, whether the TypeScript modularity refactor is done.

# Notes

Owner request 381, part A. Owning lane: mu-statesync. Related: `#1227 poll-storm-stale-thread` (page reads must never start an upstream read; server-scheduled refresh with live events), `#2278` (server side of the same night's report; owns the background-refresh budget fix — coordinate before touching it), `#2277 vibe-talk-key-scroll-freezes-11-26` (client freezes from the same report), `#624 voice-page-modularity` (closed; the refactor the owner asked about). Survey first: the table goes to the coordinator before any gate.

S3 waiver landing (2026-11-26): `afb8ecaa` (recap/digest held-copy answers and the recent-window retry) landed as `dce61a5b441abe28ebe64880365265994db88860` with `land-main --no-gate-evidence` under coordb's waiver at about 10:1x AM ET. The full gate and exclusive rerun were red only on `read-modes-browser`, reproduced on unmodified main and filed as `#2293`; every other step on the full gate but the discuss OOM `NO_RESULT` was green. Approved by coordb. The landing was already complete when the liaison's refusal of that shortcut arrived; the liaison then chose to keep `dce61a5b` only on conditions (a) #2293's read-modes disable or fix itself lands behind a passing gate, and (b) a full owner gate on main's tip (S3 included) is all green with nothing S3 touches red — failing that, `dce61a5b` is reverted at once. No deploy until both conditions hold.
