---
title: 'voice-page-action-menu: a hamburger menu for common actions and a real Voice/Channel button'
status: closed
priority: 0
issue_type: feature
assignee: cl-refactor@devbig030
labels:
- vibe-talk
depends_on:
  agent-utils-819: blocks
created_at: 2026-11-03T15:53:06.395147387+00:00
updated_at: 2026-11-03T18:43:26.911190737+00:00
closed_at: 2026-11-03T18:43:26.911190617+00:00
---

# Description

[opus 5.5] OWNER P0 (2026-11-03), part 2 of 2, after #819 lands. Owner-approved exception to the #624 voice-page-modularity linearization rule.
(c) A hamburger menu of common actions, so the floating chips stop crowding the page.
(d) The Voice/Channel toggle becomes a smaller real button.

# Design

(c) A hamburger icon just LEFT of the channel selector opens a menu of common actions, starting with Expand all / Collapse all and Summaries. Those floating chips move into the menu. Undo and Jump-to-newest stay floating.
(d) The voice/channel toggle becomes a smaller real button with a pressed visual style; its label shows the current view (Voice / Channel).
Mobile first.

# Acceptance Criteria

- The menu opens from the hamburger left of the channel selector on phone and desktop layouts; Expand all / Collapse all and Summaries work from it, and their floating chips are gone; Undo and Jump-to-newest still float.
- The toggle is a button with a pressed state and a label naming the current view; keyboard and screen-reader accessible.
- Browser checks cover both; existing layout tests updated.
