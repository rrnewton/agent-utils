---
title: 'card-root-lines: a thread card that shows its first message keeps that message''s lines'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-24T10:44:34.390624343+00:00
updated_at: 2026-10-24T10:44:34.390624343+00:00
---

# Description

[opus 5.5] The owner (rescued Slack message 1792676641, 2026-10-22, Threads view on a desk): "If we don't have a thread summary for the card and we are showing the 1st message instead... shouldn't it render properly? Why take my message with header / ----- / body and remove newlines to squish it into one line? I think if we render as a message we should do it properly." sideCardText and the Threads list's card collapse every run of white space, line ends included, into one space.

# Acceptance Criteria

A card without a summary draws its first message's text with its line ends (as the message's drawn body reads, one line per block or line break), clamped by the collapsed-lines count as before; a summarised card is unchanged. Page tests; a Chromium check of a card with a heading and body.
