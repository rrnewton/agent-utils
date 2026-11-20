---
title: 'vibe-talk-reader-one-click-jump: one click on a message link while the reader is on lands expanded and starts reading it'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- reader
- discuss
- ui
created_at: 2026-11-20T08:42:46-05:00
updated_at: 2026-11-20T09:15:26-05:00
---

# Description

[watermelon-full-preview] Owner request (228, 2026-11-20): while the reader is reading a Discuss summary aloud, jumping to a message linked from the summary takes several actions — the click drops the Discuss sheet, the reader must orient in the timeline, then tap the message text to begin reading it. The invariant the owner wants: whenever the reader is on, one click on a message link gets the top of that message on screen, expands the message, and begins reading that message from its start. When the reader is not active, current link behaviour is unchanged.

# Design

- Capture whether the reader is active at the tap, in the shared message-link jump (`jumpFromSpot` in `web/src/view/shell/navigation.ts`, which Discuss citations, in-app message links and sent-message jumps all pass through).
- After the existing seek walk lands the message (loading older windowed pages as needed), and only where the jump is still current and a row holds the message, start reading that one message from its start through the shared read-aloud owner. A message already being read restarts from its start, never toggles off.
- The landing itself is the existing one: `landOnDeepLink` opens a folded read run, and `revealRow` expands a collapsed row, puts its top under the floating clearance, and re-asserts over the following frames while the Discuss sheet's drop to half height settles, so the final scroll is measured at the settled layout.
- Click-to-reading-start timing is logged for the jump.

# Acceptance Criteria

- Reader active plus one click on a message link (Discuss summary, maximized or half-height Discuss, phone and desktop) lands with the message top on screen (below the header, above the Discuss sheet), the message expanded, and the reader reading that message from its start. No second tap and no manual scroll.
- Reader not active: link behaviour is exactly as before.
- A folded or collapsed target is expanded; a target outside the loaded window is loaded by the seek walk, then scrolled to, then read.
- Page unit tests cover the decision (active/inactive, landed/not landed, message/no message); a browser test through the real router on a fixture is ready to run and asserts top visible, expanded, and reader position at that message, with timing logged.

# Notes

Owner-priority, deployed on landing. Browsers cannot run on this host right now (missing system libraries since the 2026-11-19 reprovision), so the browser test and screenshot walk are prepared ready to run and the lane reports blocked only on that; all other tests run locally. Test instances only; never the live instance.

## Resolution

[watermelon-full-preview] Landed on main as ff5d2bb3 (feature 64c77a82, replayed over 30d7ee00 under the coordinator's gate waiver). One click on a message link while the reader is active now captures that fact at the tap in the shared jump, and after the seek walk lands the message — expanded by the existing landing, its top re-asserted on screen while the Discuss sheet settles — the shared read-aloud owner starts reading that message from its start, restarting rather than toggling off where it is already that message; click-to-reading-start timing is logged. Reader off is unchanged. Page unit tests cover the decision and the jump; the browser test through the real router passed on a Chromium host at the gated tree (51 ms phone, 39 ms desk click-to-reading-start), and the Discuss screenshot matrix (light/dark, phone/desktop, keyboard open/closed) passed there too. The full local gate on the gated tree had only browser-family environment reds, waived by the coordinator with that evidence.
