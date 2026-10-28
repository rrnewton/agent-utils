---
title: 'server-side-search: Phase 0, honest scope on the page and All without the ten-thread bound'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- search
depends_on:
  agent-utils-521: parent-child
created_at: 2026-10-28T01:27:47.726771237+00:00
updated_at: 2026-10-28T01:27:47.726771237+00:00
---

# Description

[opus 5.5] Part of #521 server-side-search (owner 2026-10-27 18:21, Q1: yes to Phase 0 now; All should drop the "10 threads per load" bound). Slack history returns top-level messages only and replies need one call per thread, which is why All capped the threads it read per page. Want: Slack's All reads the replies of every thread rooted on the page, taking a thread's replies from the server's cache where the cache's copy matches the root just listed (same reply count, same latest reply) under the serving gates (verified account, not suspended, not in doubt, no dropped offer since), and upstream otherwise; the notice and the page's summary say only what is still not covered (threads longer than a page of replies).
