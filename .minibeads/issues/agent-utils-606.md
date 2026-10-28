---
title: 'server-side-search: search results follow the view (Main, All, Threads)'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- search
depends_on:
  agent-utils-521: parent-child
created_at: 2026-10-28T01:27:58.534995808+00:00
updated_at: 2026-10-28T03:35:35.765999168+00:00
closed_at: 2026-10-28T03:35:35.765999038+00:00
---

# Description

[opus 5.5] Part of #521 server-side-search (owner 2026-10-27 18:21, Q2). Main (Cmd-1): a thread stands for all its children, so any hit in the thread keeps it. All (Cmd-2): threads unpacked inline, every message a candidate on its own. Threads view: a thread with any hit selects the card, and unpacking it shows only the hits with folded regions between them.
