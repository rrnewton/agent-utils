---
title: 'vibe-talk-top-anchor-policy: user-caused height changes keep the touched message''s top still, even at the end of history'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T14:39:00-05:00
updated_at: 2026-11-21T14:39:00-05:00
---

# Description

The owner: "Clicking either in the beginning or end of the message keeps the BOTTOM of the message fixed and the messages ABOVE it move... why is that? What is the policy? In this case we are scrolled to the END of the message history with the text input window on the bottom of the screen. Still, I would rather in this case the top of the message I click stay FIXED and the area below MOVE. I want to bias it towards static top / dynamic bottom wherever that is possible. Even if that pulls UP the message-send text box so it is no longer at the BOTTOM of the screen."

# Design

One anchoring rule, in one place: for any user-caused height change (selecting or expanding a message, fold or unfold, action row, quote, reply box), the top of the touched message stays where it is and everything below moves, including at the end of history, even if the composer lifts off the bottom. Stick-to-bottom applies only to newly arriving messages. Record the policy in the view-stability design notes.

# Acceptance Criteria

- The touched message's top offset changes by 0 px in every frame for each user-caused height change, at the end of history and mid-list, on phone and desktop, windowed on and off.
- Arriving messages still stick to the bottom when the view is at the end.
- Deterministic frame- and condition-based browser checks; screenshots.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
