---
title: 'voice-page-jump-to-reading: a chip that brings the message being read back into view'
status: open
priority: 1
issue_type: feature
assignee: cl-refactor@devbig030
labels:
- vibe-talk
created_at: 2026-11-03T23:26:28.561473656+00:00
updated_at: 2026-11-03T23:26:28.561473656+00:00
---

# Description

[opus 5.5] OWNER request (2026-11-03): while read-aloud plays (or is paused), the message being read can scroll off screen, and there is no quick way back to it.

# Design

A "jump to message being read" chip, styled like Jump to newest, shown if and only if the message being read aloud is off screen: an IntersectionObserver on the lit row shows the chip when the row leaves the viewport and hides it when the row is visible again; a tap scrolls the row into view, centred. Works with read-aloud playing or paused.

# Acceptance Criteria

- Browser tests: the chip appears when the lit row is scrolled off screen, hides when it comes back, and a tap brings the row into view centred; also with read-aloud paused; phone and desktop layouts.
