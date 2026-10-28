---
title: 'cache-first-older-reads: step-back (older) and counts reads still go to Slack; one page repeats an older read every 45 s'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
depends_on:
  agent-utils-622: related
  agent-utils-630: related
created_at: 2026-10-28T14:15:34.246403792+00:00
updated_at: 2026-10-28T16:02:17.311834718+00:00
closed_at: 2026-10-28T16:02:17.311834567+00:00
---

# Description

[opus 5.5] Live on devbig014 after #622 cache-first-reads (main 725b6f49, 2026-10-28 10:04 ET), first 10 minutes: 37 timeline reads, 0 5xx; served_from=cache/cache-first for reset/poll; but read_reason=older served_from=upstream on C0CAAAF16QK at 14:10:41, 14:11:28 (21.2 s), 14:11:41, 14:12:26, 14:13:11, 14:13:56, 14:14:41 UTC (~2.6 s each, every ~45 s: a repeating step-back from the page, possibly a loop), and read_reason=counts served_from=upstream 8.6 s on C0C91RW5FAR. The cache holds full history, so older pages and counts should be answered from it (owner rule: the page never waits on Slack when the server holds the view; #630 state-sync-model). Find and stop the 45 s repeating older read; serve older and counts from the cache with Slack behind.
