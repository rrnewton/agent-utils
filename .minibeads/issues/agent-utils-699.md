---
title: 'vt-send-priority: the owner''s sends go before background reads while Slack is limiting'
status: open
priority: 0
issue_type: bug
created_at: 2026-10-29T20:57:25.873573252+00:00
updated_at: 2026-10-29T20:57:25.873573252+00:00
---

# Description

The owner's messages sent from the vibe-talk page are slow to post on the live Slack instance:
one send on 2026-10-29 took upstream_ms=34866 (outcome posted).

The owner's Slack identity is limited to a per-person token bucket (about 120 Slack calls a
minute, every method and every host together); a refusal comes back as a rate limit. Measured
that minute: sent=27 limited=22 refused=8, almost all of it conversations.replies and
conversations.history reads. The slow send spent 30 s in its own pre-post read of the thread
(the #315 removed-reply-target check), refused at :02, :12 and :22 behind a ~10 s Retry-After
while background reads kept the same bucket empty; chat.postMessage itself then took 4.9 s.

Fix, in the server only (the page is frozen for #624):

1. Writes first. chat.postMessage and the other owner-initiated writes (reactions, read marks),
   and the reads a send makes before posting, take priority. While the provider is limiting
   (a rate-limited answer on any method in the last ~60 s), background reads (relist, the live
   poller, behind-reads the cache already answers, naming, backfill) are paused, so the bucket
   goes to sends.
2. A send never waits behind a background read's Retry-After: it tries at once and retries on
   its own short schedule.
3. The minute's budget line says writes vs reads and how many reads were paused.

Tests: a send while the provider refuses reads still goes out first; background reads back off
while limited.
