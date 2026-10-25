---
title: 'search-whose-toggle-hint: say whose messages the search toggle shows, on hover and on press'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- ui
- search
created_at: 2026-10-25T21:58:49.931042104+00:00
updated_at: 2026-10-25T22:55:12.740972974+00:00
closed_at: 2026-10-25T22:55:12.740972864+00:00
---

# Description

[muse watermelon] Owner (2026-10-25): the search bar's whose-messages toggle (Everyone / Mine / Theirs, #251 author-filter) is unintuitive; the icons are liked once understood but their meaning is forgotten. Add (1) a hover/focus tooltip naming what the toggle does and its current state, also as aria-label/title for keyboard and screen readers (on touch, a long-press or the chip below covers it); (2) on each toggle, a small transient chip under the control, reusing the page's existing chip/toast style, saying 'My messages', 'Their messages' or 'Both', fading after ~1.5s (reduced motion: no animation), not shifting layout. Page tests for the labels and chip per state; a Chromium screenshot at 393x852 and 1280x800. Help/README line if the toggle is documented.

# Acceptance Criteria

Tooltip visible on hover and keyboard focus naming the control's purpose and current state; aria-label/title carry the same. Each press shows the chip with the new state's words ('My messages' / 'Their messages' / 'Both') under the control for ~1.5s without shifting layout; no animation under prefers-reduced-motion. Page tests pin labels and chip per state; Chromium screenshots at 393x852 and 1280x800 show the chip. Gate green.
