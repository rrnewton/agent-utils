---
title: 'voice-page-dictation-polish: hide the keyboard while dictation listens and show a mic badge'
status: closed
priority: 1
issue_type: feature
assignee: cl-refactor@devbig030
labels:
- vibe-talk
depends_on:
  agent-utils-828: blocks
created_at: 2026-11-03T22:00:33.289746695+00:00
updated_at: 2026-11-04T00:13:26.437418813+00:00
closed_at: 2026-11-04T00:13:26.437418612+00:00
---

# Description

[opus 5.5] OWNER request (2026-11-03 16:15 ET), voice-mode polish after #821 and the nav-row fix: while dictation listens, the virtual keyboard covers the reply, and there is no sign that the page is listening.

# Design

- Hide the virtual keyboard while dictation listens: keep focus and caret in the reply box but set inputmode="none" (or equivalent) while speech recognition runs; restore it on a tap in the box, on stop, and on error.
- A microphone indicator while listening: a small round reaction-style badge with a mic glyph dangling off the LOWER-LEFT corner of the floating quoted-reply box; it may pulse gently, respecting prefers-reduced-motion; tapping it stops listening; it is removed when recognition ends.

# Acceptance Criteria

- Tests with the stand-in recognizer: inputmode toggles to none while listening and back after; the badge is shown, positioned at the lower-left corner, and removed; tapping it stops listening.
