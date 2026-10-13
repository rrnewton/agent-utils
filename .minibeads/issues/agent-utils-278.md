---
title: 'page-live-reads: stop re-reading the channel for every live event and after a timed-out read'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-13T20:05:29.708859907+00:00
updated_at: 2026-10-13T20:05:29.708859907+00:00
---

# Description

Remote-ops audit (2026-10-13) items W6 and W7. W6: the page re-reads the channel after every arriving live message (one timeline read each) and after every edit or delete (two: Main and the side column), and polls every 45 s even while a push stream keeps it current. Add to LiveMessageEvent the facts the re-read exists to fetch (the thread record, the root's reply_count and updated_at), apply message_update and message_delete from the payload, debounce to at most one delta per 10 s only when a fact is missing, and while attached with push delivery stretch the 45 s poll to the 15 min reconcile, polling on a gap, reset or reconnect (45 s kept while detached). W7: after a view's read times out at 20 s, send nothing more for that view until the server's 50 s bound has passed or a live event arrives. Each change proved by a page test that counts timeline requests per user-visible action.
