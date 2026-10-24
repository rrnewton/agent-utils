---
title: 'thread-card-fit: thread cards shrink to fit their content'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-24T10:13:07.277929443+00:00
updated_at: 2026-10-24T10:13:07.277929443+00:00
---

# Description

[muse watermelon] The owner (phone screenshot of the Threads view, 2026-10-24): "Looks like thread cards are fixed height and waste space. They should instead shrink to fit." A one-line card reserves the full collapsed-lines height, leaving a large gap between its title and its footer chips. Wanted: every thread card variant (packed, summary, unpacked, highlighted/expanded, heading card, with and without the replies chip, phone and desktop, every read mode) sizes to its content: the collapsed-lines count stays a maximum, never a fixed or minimum height. Tap targets and footer chips keep their sizes.

# Acceptance Criteria

A short card is no taller than its words plus its facts line at phone and desktop sizes; a long raw card still clamps at the collapsed-lines count (one line at 0); summarised and highlighted cards still show whole text without reserved empty lines; chips keep thumb height on a phone; page tests plus a Chromium walk with screenshots at 393x852 and desktop.
