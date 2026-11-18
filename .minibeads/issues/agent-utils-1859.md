---
title: 'vibe-talk-ready-toast-covers-text: the Ready toast after a load covers message text'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
created_at: 2026-11-18T13:15:00-05:00
updated_at: 2026-11-18T13:15:00-05:00
---

# Description

[watermelon-full-preview] Owner-UI follow-up from the #1849 reload shots: after a load or reload the "Ready." toast floats mid-screen over message text (visible in the after-reload screenshots, phone and desktop). It says nothing the reader must act on, but for its whole stay it covers the message under it.

# Design

- An idle confirmation ("Ready." and its lowercase twin, the words `IDLE_STATUS_WORDS` already singles out for the short clock in `createShellNotices`) is announced but not floated: the status line carries `data-idle` while it says one, and the stylesheet waits it out (`visibility: hidden`, the #1837 Welcome-wait idiom) so its geometry, aria-live text and dismiss timer are untouched.
- Action and progress words are unchanged: the attribute goes, the pill floats as before, on the full clock.

# Acceptance Criteria

- After a load or reload, no "Ready." pill stands over message text, at phone or desktop; a screen reader still announces it.
- An action or progress status still floats visibly and keeps the full dismiss clock.
- Deterministic tests (condition waits only) plus the screenshot matrix (light/dark, keyboard open/closed, phone 412x915 and desktop 1280x800, Discuss half-pane on phone, before/after reload) captured on a test instance and inspected.

# Notes

Owner-priority, deployed on landing. Builds on #1837 vibe-talk-status-pill-avoids-text (the short idle clock and the Welcome wait).
