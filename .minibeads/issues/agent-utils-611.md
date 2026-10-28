---
title: 'timeline-read-failures: timeline reads answer 502 for a large share of reads once full backfill shares Slack''s rate budget, and the page sits on Loading messages'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- cache
created_at: 2026-10-28T04:14:34.774119473+00:00
updated_at: 2026-10-28T04:14:34.774119473+00:00
---

# Description

[opus 5.5] Owner, 2026-10-27 20:57 PDT: "My vibe talk app is stuck. It is stuck loading messages."

Evidence (the coordinator, from the deployment's app.log, main 546a72f0, Slack provider, `[message_cache] serve = true`): GET /api/v1/channels/{id}/timeline answers 502 after 9-25 s for a large share of reads: 0/232 in the hour before backfill = "full" was turned on (#595 cache-full-history), then 61/348, 103/364, 156/402, 95/360 per hour. Failing read reasons: side, threads, enter, poll, live, counts, reset, mutation. Beside them: Slack rate limits on conversations.history / conversations.replies ("gave up after 5 attempt(s)"), and "a page answered from the message cache could not be refreshed" warnings. The hourly upstream profile: fetch_timeline n=358 calls=2008, about 5.6 Slack calls per timeline read. After backfill went back to "recent" the rate-limit lines nearly stopped, but 9/40 timeline reads still answered 502, and no log line said why: the cause of a 5xx (ApiError chat_error) is never logged.

## Diagnosis

1. **No 5xx says why.** `ApiError` turns any upstream `ChatError` except Refused/CursorExpired into 502 chat_error, and nothing logs its cause. The access line has only the status.
2. **Most timeline reads cannot be answered from the cache, and every one that fails upstream is a 502.** `ops::timeline` serves only a NEWEST page that (a) wins the per-read refresh claim and (b) is serve-eligible: on Slack that means Main alone (`shadow::serve_eligible`: Flat, Thread and Threads are never served on Slack). A newest read whose refresh claim is already taken by an earlier serve's background refresh goes upstream synchronously. A forward read (`after=`), a step back, and any read of All, a thread or the thread list always go upstream. Any upstream error there is a 502, though the cache often holds the scope (Main, a thread).
3. **All costs one call per thread, and one failed thread fails the whole read.** Slack's All (Flat) is `conversations.history` plus one `conversations.replies` per thread rooted on the page that the cache does not hold whole (#603 server-side-search removed the ten-thread bound). The replies are collected with `try_collect`, so one rate-limited thread fails the whole read with a 502. On Slack only Main and one thread are read forward natively. All and the thread list catch up through `generic_since`, which reads the full newest page (up to 99 rows plus the per-thread replies) on every poll, live re-read, counts read (the page reads All behind Main to count unread replies) and side read. That is where ~5.6 calls per read comes from, and why counts, side and threads are the reasons that fail.
4. **The rate budget is not shared by priority.** Each Slack call waits out a 429 on its own (5 attempts, a 30 s budget per call). Backfill gives up at the first limit and holds off for 5 min after one (#602 history-settings-and-limits), but by then interactive reads have already hit the limit. Full backfill (#595) pushed conversations.history/replies over Slack's per-method tier, and the interactive reads, each costing several calls, lost.
5. **Load makes it worse.** On a loaded host the provider bridge answers slowly, so reads take 9-25 s before failing. The page gives up at 20 s and holds its polls for 50 s, which looks like an endless "Loading messages…".

## Plan

Landing 1 (unblocks the owner): log every 5xx once at WARN with its cause (error code and detail, the upstream operation that failed, its outcome and elapsed time, the read reason). Never let an upstream failure or slowness of a read the cache can answer become a 502: answer from the cache, marked `refresh_failed` with `as_of`, and the page says "Refresh failed · showing messages from HH:MM". Concurrent reads of one scope share one upstream read. Reads the cache cannot answer still fail, but visibly.

Landing 2: cut calls per read (All's replies within a bound and taken from the cache when held; counts, side and poll reads served from the cache plus live ingestion; refreshes coalesced and spaced per channel), and one shared Slack budget by priority (interactive, then live, then summaries/naming, then backfill), so that full backfill can be turned back on safely.
