---
title: 'naming-budget: read each channel at most once per 30 s to name threads, and pause a failing channel'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-14T05:07:54.868290395+00:00
updated_at: 2026-10-14T05:08:03.446278361+00:00
closed_at: 2026-10-14T05:08:03.446278221+00:00
---

# Description

[opus 5.5] remote-ops-audit W5, parts (b) and (c): a per-channel naming read budget (summaries.thread_naming_interval_seconds, default 30 s) and a 10-minute pause of a channel's naming reads after a provider failure there. A thread is named only from a read of it made at its turn; part (a), naming from rows already held in read-ahead, is dropped.
