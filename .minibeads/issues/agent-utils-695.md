---
title: 'vt-behind-reads: no Slack read behind a page poll the live feed already answers'
status: closed
priority: 2
issue_type: task
created_at: 2026-10-29T16:26:26.598630479+00:00
updated_at: 2026-10-29T16:29:13.796393616+00:00
closed_at: 2026-10-29T16:29:13.796393456+00:00
---

# Description

Every page poll answered from the cache first (#622, #688 vt-timeline-latency) begins an upstream read
of its view behind the answer. On the live Slack instance those behind-reads were the largest single
consumer of the owner's Slack API Proxy budget (3-13 calls/min of a 120/min per-person bucket), though
for a live-fed channel the live poller re-lists Main every minute and the relist re-reads recent
threads, so most of them re-read what the cache was just told.

Skip the behind-read when the channel is live-fed and the cache's own record shows the view's newest
end listed within a short freshness window; keep the answer marked as the cache's (refresh_failed) so
the page answers for nothing it lacks, and keep the thread list's own refresh (its way back is Slack's).
Test that a live-fed channel's freshly listed view spends no Slack call on a page poll, and that a stale
one still reads behind.
