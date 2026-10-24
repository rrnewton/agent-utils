---
title: 'cache-shadow-missing-row: explain and fix the shadow''s one missing row per window'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- message-cache
created_at: 2026-10-24T13:27:10.383524164+00:00
updated_at: 2026-10-24T13:27:10.383524164+00:00
---

# Description

[opus 5.5] On the Slack instance the message cache runs in shadow mode (serve off), and every long-lived process window shows exactly one missing-row disagreement: 2026-10-23 88 compared / 87 agreed / missing_rows=1; 2026-10-24 (process from about 07:56Z) 56 compared / 55 agreed / missing_rows=1; also 2026-10-23 13:53Z 64 / 63 / 1. Every other window agrees fully. The hourly "message cache figures" line is cumulative and says nothing about which row was missing, so the cause cannot be read from the log. The owner wants serve switched on once the shadow proves it correct, and a would-serve with a missing row would be a wrong answer.

# Acceptance Criteria

Every shadow disagreement writes one rate-limited, content-free WARN line: channel, scope, the window and the read it rests on, asked time, read reason, and each differing message id with its times, what the file holds of it and how it reached the cache. The comparison never counts as missing a message the answer never claimed to hold, and still counts every one it did. The likely causes are reproduced in tests, and any real hole in the rule is fixed.
