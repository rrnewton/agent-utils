---
title: 'state-model-formal-round: formalise and model-check the page/server state and sync model'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
depends_on:
  agent-utils-630: blocks
  agent-utils-624: blocks
created_at: 2026-10-28T15:13:15.905593528+00:00
updated_at: 2026-10-28T15:13:15.912516988+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 11:00 ET: before new mutation kinds (e.g. undo-send), expand and formalise #630 state-sync-model to cover revisions, documents (#628), sends and outcomes, with model checking (a small TLA+/Quint model or property tests over the sync rules). After #624 voice-page-modularity.
