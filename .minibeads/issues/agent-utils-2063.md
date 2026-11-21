---
title: 'vibe-talk-quote-reply-view-stability: quote-replying in two-column thread view must keep the quoted post still'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T12:26:00-05:00
updated_at: 2026-11-21T12:26:00-05:00
---

# Description

The owner, reading a thread in two-column mode and replying to a long message: "when i quote reply ... some junk updates lower on the screen and the post I'm replying to DOES NOT STAY STILL on the screen. This violates our view stability principle I believe".

# Design

Find what moves the quoted post when quote-reply starts (composer opening, quote insert, count or chip refresh, windowed re-slice) and anchor every such update to that post, so its top offset does not change.

# Acceptance Criteria

- In two-column mode with a thread open, starting a quote-reply on a long message leaves the quoted post's top offset unchanged (0 px), at phone and desktop widths, with windowed lists on and off.
- Covered by a deterministic, frame- and condition-based browser check (no wall-clock budgets).

# Notes

Related earlier anchoring work: #1983, #2008, #2009. Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
