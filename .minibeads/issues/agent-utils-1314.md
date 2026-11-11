---
title: 'unread-step-keys: Shift+Up/Down moves the selection to the previous or next unread message'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-11T16:08:05+00:00
updated_at: 2026-11-11T16:08:05+00:00
---

# Description

[opus 5.5] Owner request (reply 378): "when I use arrow keys I step up and down by one message... If I press shift-up / shift-down it currently does nothing. I would like to use shift-up/down to navigate ONLY unread messages. This makes it easier for me to clean out the unreads in a thread."

# Design

- Shift+ArrowUp / Shift+ArrowDown move the keyboard selection to the previous or next UNREAD message in the current view (thread or channel), skipping read ones. Plain arrows keep stepping one message.
- At either end, the selection stays put and a brief "no more unread" hint shows.
- Works under Show and Fold; listed in the shortcuts help.
- The landing scroll goes through the single scroll owner (#1251 scroll-single-owner), with no direct scroll write.
- Owner fast-track: path-selected gate, deployed on landing.

# Acceptance Criteria

- A desktop (1280x800) browser walk over mixed read and unread rows: Shift+Down visits only unread rows in order, Shift+Up goes back the same way, and at the end the selection stays and the hint appears. Plain arrows are unchanged.
- The page suite covers the key binding and the help entry.
- No wall-clock waits in the test (owner rule 346).
