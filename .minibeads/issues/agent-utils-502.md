---
title: 'cache-serve-coverage: the cache stops proving the newest page after a restart, and read latency is not measured'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- message-cache
created_at: 2026-10-25T12:05:50.197592342+00:00
updated_at: 2026-10-25T12:28:32.997086432+00:00
closed_at: 2026-10-25T12:28:32.997086081+00:00
---

# Description

[opus 5.5] On the Slack instance (serve = true since 2026-10-25 02:46 UTC) the hourly figures showed shadow_asked=30 shadow_would_serve=0 for eight hours after the restart, and the process before it had would_serve 0 of 133-247 for five hours, where the one before had served 87% in shadow. Upstream timeline reads take p50 4.3 s, p90 5.4 s; the cache records no timing for the reads it answers, so the owner cannot be told how serving changes p50/p90/p99.

Evidence from the instance (read only): channel.dropped_ms of two channels at 02:46:17-18, a second after the start and after the account check answered (02:46:16); the first Walk claim of the process listed at 08:43, six hours after the start; coverage came back once that walk ran (11:20 onward: 162 of 263 asks servable, 81 served, 161/161 agreed, 0 missing rows; misses gap=93 suspect=8 since the start). The relist and poller read [chat] default_fetch_limit = 40 rows while the page asks 50, so what lies under the relist's page is proven only by older claims, the walk's first among them.

Cause: main starts the backfill, the live poller and the relist with the server, before each provider's account has been checked against the cache file. A fill begun then writes no rows and marks its channel in doubt, and the backfill's whole-walk listing begun then proves nothing; the walk is marked over and is not walked again for six hours (BACKFILL_REWALK).

Asks: shadow_asked counts only readers' reads (a page's newest timeline page in Main or one thread, windows, lookups, pages, counts), never the relist's, poller's or backfill's; the flat 30 was eight hours with no page open (the access log has no timeline read from 04:00 to 10:00 UTC), and the Slack Flat view the owner uses for one channel is never asked.

# Acceptance Criteria

The backfill begins no walk before the provider is verified and starts a walk over when it loses the check; the poller and relist wait (bounded) for the first checks. A test restarts a server on the same file with the backfill stepping before the check and fails before the fix (no Walk claim written). The hourly line carries misses by reason and per-consumer read latency (cache, upstream, would: n, p50, p90, p99); diagnostics carry the same per consumer; the README says how to read them. Content-free.
