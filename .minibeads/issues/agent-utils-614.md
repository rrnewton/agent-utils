---
title: 'timeline-read-failures: side reads still 502 under Slack rate limits'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- cache
created_at: 2026-10-28T07:11:52.122480559+00:00
updated_at: 2026-10-28T08:07:45.841200115+00:00
closed_at: 2026-10-28T08:07:45.841200025+00:00
---

# Description

[opus 5.5] Follow-up to closed #611 timeline-read-failures. Its two landings (ebf80213 answer a failed or slow read from the message cache; cde88ed5 bound Slack's All, keep the thread list's scan, one Slack budget by priority) went live 2026-10-28 02:56 ET.

Evidence (the coordinator, read-only from the deployment's app.log, Slack provider, `[message_cache] serve = true`, backfill paused):
- ebf80213 window (44 min): 230 timeline reads, 28 answered 5xx, ALL with read_reason="side"; 8 timeline_stand_in lines; 91 reads slower than 10 s; 18 Slack RATE LIMIT lines.
- cde88ed5, first 5 min: 54 timeline reads; side reads 6x200 / 12x502; 11 stand-ins; 27 slower than 10 s; 14 RATE LIMIT lines.
- Typical: `status=502 millis=32744 read_reason="side" ... Slack RATE LIMIT (this route's bucket) on conversations.history: gave up after 5 attempt(s)`. Durations 16-47 s against timeline_fallback_ms = 10 s. Live ingestion backs off on conversations.history too.

## Diagnosis
- A `side` read is the right column: the thread list (`view=threads`, read beside Main at most once a minute per tab) or the side column's thread. The thread list is never stood in for: `Sink::stale_timeline` refuses the Threads view ("the cache does not keep the thread list"), though the cache keeps a thread table (record per thread: root, reply count, last activity, moved forward by every reply live ingestion delivers). So once the read is past timeline_fallback_ms the request waits for upstream, which is in the Slack client's rate-limit loop (up to 30 s of Retry-After waits and five attempts), and answers 502 after 16-47 s.
- The thread list is also the largest `conversations.history` spender: each newest list (and each forward catch-up, which reads the newest list) scans up to ten history pages (THREAD_SCAN_PAGES), once a minute per tab per channel, whatever live ingestion already told the cache.
- The live poller's history reads carry no read reason, so they count as interactive and cannot be told apart in the figures.

## Plan
1. The Threads view's stand-in: a thread list put together from the cache's thread records and held roots, newest activity last, for Slack.
2. Side and counts reads give up at the first rate limit (and never send into a Retry-After), so the cache answers them at once; one the cache cannot answer fails within timeline_fallback_ms instead of waiting out the provider.
3. With the channel live-fed and serving on, the thread list and the counts read of All are answered from the cache while an upstream read of that view finished within five minutes (or one is running); otherwise they go upstream (with the stand-in behind them), so side and counts reads stop spending the history bucket on what live ingestion already delivered.
4. The live poller's reads carry their own reason (live-poll, Live priority).
5. One INFO line a minute: Slack calls in the last minute by method and read reason, with rate-limited answers and refusals, so the effect can be checked from the log.
6. Fake-provider tests: side reads under a rate-limited conversations.history answer from the cache within the fallback (no 502), call counts per caller.
