---
title: 'voice-toggle-colours: Voice is the lit blue state and Channel the darker one'
status: open
priority: 1
issue_type: bug
assignee: cl-refactor
created_at: 2026-11-04T19:00:42.662342050+00:00
updated_at: 2026-11-04T19:00:42.662342050+00:00
---

# Description

The owner, on the Voice/Channel view toggle: "flip their colors. I want the channel view to be the darker one. And the voice view to be the lit up blue one".

# Design

- The Voice/Channel toggle shows Voice as the lit (blue, pressed) state and Channel as the darker state.
- aria-pressed follows: pressed = Voice. Labels and keyboard behaviour are unchanged.

# Acceptance Criteria

- Page tests assert the colours/classes and aria-pressed for both states; a screenshot check if one covers the toggle.
- Gate passes.

# Notes

Owner ask lost while the coordinator was down (2026-11-03/04), relayed by the lead 2026-11-04. Before more #624 batches.
