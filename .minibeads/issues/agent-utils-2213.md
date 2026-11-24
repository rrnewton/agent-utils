---
title: 'vibe-talk-long-code-lines-cut-off: long code lines are cut off at the card edge'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T09:30:00-05:00
updated_at: 2026-11-24T09:30:00-05:00
---

# Description

In a message card, a long line inside a code block runs to the card's right edge and is cut off: the rest of the line cannot be read in place. Seen in the #2155 A2 preset shots (a one-line Rust example in a sent message). Pre-existing; not caused by the theme work.

# Design

- Give code blocks the page's usual long-content treatment (horizontal scroll inside the block, or wrapping) without moving the card or the list, consistent with how other wide content is handled.

# Acceptance Criteria

- A message whose code block holds a line far wider than a phone card shows the whole line (by scroll or wrap) at phone and desktop sizes, in light and dark, with no page-level horizontal scroll.

# Notes

Filed by mu-themes at coordb's direction during #2155 A2 review. Low priority.
