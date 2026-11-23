---
title: 'vibe-talk-linked-jump-centers-target: a linked-message jump centers the target instead of top-aligning it'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-23T05:55:00-05:00
updated_at: 2026-11-23T08:01:02-05:00
closed_at: 2026-11-23T08:01:02-05:00
---

# Description

Owner request: when a summary, search or Discuss link jumps to a linked message, the page currently scrolls the target to the top of the message list. The owner wants it in the middle: since the page controls the scroll, center the target message vertically in the visible list area.

# Design

- On every linked-message jump (summary citations, Discuss search hits, Discuss follow-up links), center the target in the visible list viewport instead of top-aligning it.
- The visible area is what the reader actually sees: on phone with the Discuss sheet open, the list area above the sheet; with the on-screen keyboard open, the shrunk viewport.
- Clamp at the list ends (no blank space past the first or last message); a target taller than the viewport aligns its top.
- Keep the Linked message highlight and the stable-screen behaviour of the in-place jump (#2080) and the top-anchor policy (#2076); change only the target position, not the anchoring rules.

# Acceptance Criteria

- Page tests for the computed position: phone and desktop, Discuss open and closed, keyboard open and closed, a target near each list end.
- Screenshot matrix with before and after of one jump, phone and desktop, light and dark, Discuss open, keyboard open and closed; the target's rect sits at the middle of the visible list area (a sidecar with the numbers).

# Notes

Owner UI request (fast-track); filed by the coordinator.

## Resolution

[watermelon-full-preview] Landed on main at cb64dd4a (gated db75d925, coordinator waiver for the #2158 pin-picker time bomb and a load-flaked reader-resume step). Every linked-message jump now centres the target in the visible list area — above a phone Discuss sheet, inside the keyboard-shrunk viewport — clamped at the list ends, with taller-than-viewport targets top-aligned; the Linked message highlight, the #2080 in-place stability and the #2076 top-anchor holds are unchanged. Evidence: full page JS suite, the real-server summary-link walk with per-landing centred assertions and rect sidecars, and the screenshot matrix (scenes 73–76 plus the new 90-discuss-centered-jump) before and after landing. The read-modes browser check reported failing against this landing passes repeatedly on the landed tree and on current main (its gate failure was a 10 s thread-list wait under load).
