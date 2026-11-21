---
title: 'vibe-talk-thread-picker-grid-toggle: thread picker: fixed views as a 2x2 grid on phone, toolbar button toggles, explicit close'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T13:55:00-05:00
---

# Description

The owner likes the thread list overhaul but: "I don't like the haphazard line wrapping with the two rows of buttons for the four buttons for the fixed views ... they should be displayed as a 2x2 grid filling that space. I want maximum clarity and clickability on mobile" and "If clicking the thread button on the toolbar at the bottom of the screen pops it open. Clicking it again should certainly close it. We could also have an x button for it explicitly."

# Design

Phone: the four fixed views as an equal-width 2x2 grid filling the width with large targets; desktop keeps one row when it fits. The toolbar thread button toggles the popup; an explicit close button; outside click still closes.

# Acceptance Criteria

- Grid and toggle behaviour covered by browser checks; screenshots phone and desktop, light and dark, open and closed.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
