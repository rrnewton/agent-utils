---
title: 'send-order: messages to one destination leave in the order they were written, through retries'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-13T18:39:18.354243759+00:00
updated_at: 2026-10-13T18:39:18.354243759+00:00
---

# Description

The owner, 2026-10-13: "I am having very delayed sends of my messages and often failures: 'Sending… Google Chat could not answer: Google Chat message send failed. Retrying in 45 s.' ... And then when it retries it ends up sending messages OUT OF ORDER. I.e. I write A then B but B lands first."
