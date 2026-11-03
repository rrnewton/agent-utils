---
title: 'voice-page-backstage-toggle: hide or show coordinator trace posts per channel'
status: open
priority: 1
issue_type: feature
assignee: cl-refactor@devbig030
labels:
- vibe-talk
depends_on:
  agent-utils-832: blocks
created_at: 2026-11-03T22:00:33.315356630+00:00
updated_at: 2026-11-03T22:00:33.323062265+00:00
---

# Description

[opus 5.5] OWNER request (2026-11-03 16:29 ET), page work (an owner-requested exception like #819-#821), after the React work: let the owner hide the coordinator's trace posts ("Backstage") from the channel view and from read-aloud. Depends on #832's metadata.

# Design

- Ingest reads the agentctl.trace metadata (#832). Fallback: recognise the daily trace thread by its root ("<agent> main session trace, <date>").
- A menu item "Hide backstage" / "Show backstage", remembered per channel; shown by default until the owner says otherwise.
- Read-aloud skips the trace label line.

# Acceptance Criteria

- Browser test: the toggle hides and shows trace posts and threads, and the choice is remembered per channel.
- Ingest test: the metadata is recognised; the root-title fallback works.
