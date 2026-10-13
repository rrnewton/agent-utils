---
title: 'naming-throttle: name threads from rows already read, read at most one thread per channel per 30 s, and stop a failing channel''s naming reads for 10 min'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-13T20:01:45.302303854+00:00
updated_at: 2026-10-13T22:42:05.068364398+00:00
---

# Description

Remote-ops audit (2026-10-13) item W5, row 3 of its section 2. Background thread naming reads one thread per due thread, serially, paced only by the model limit; on the Google Chat bridge each such read is a whole-space rebuild of P CLI pages, and naming made about 146 of them in 2.5 h (about 40 % of the adapter's timeline load in its busiest hour, 78 of them failed). A failed read only backs off that one thread, so every other due thread of a failing channel is still read. Fix: (a) name from rows this server already holds - the thread list's own root for a thread with no replies, or read-ahead's copy of the thread from a Thread or Flat page read in the last 30 s; (b) a per-channel naming read budget, at most one thread read per 30 s by default; (c) after a provider 5xx, timeout or rate limit on a naming read, no naming read of that channel for 10 min. Proof: tests count upstream fetch_timeline(view=thread) calls per user-visible action with testing::CountingChat - 20 due threads over a paused-clock minute make at most 2 reads; a 502 makes 0 more naming reads of that channel for 10 min; a thread held in read-ahead is named with 0 reads.
