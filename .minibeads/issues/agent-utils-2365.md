---
title: 'vibe-talk-jump-toast-covers-landed-row: the "Here is the linked message." toast covers the row a link jump lands on'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T04:55:00-05:00
updated_at: 2026-11-27T06:20:00-05:00
closed_at: 2026-11-27T06:20:00-05:00
---

# Description

Found by the advisory design review of #2350 vibe-talk-back-toast-covers-returned-row, in the #2261 summary-link walk's landing frames (phone 412 and desk 1280, light and dark, Discuss open). After a summary-citation jump the page says "Here is the linked message." on its floating status toast, and the toast is drawn over the very row the jump landed on.

- Phone: the toast covers the landed row (R5 in vibe-talk/docs/ui-rules.md: status must not compete with content).
- Desk with Discuss open: the toast runs under the Discuss panel. Its text is cut to "...messag" and its dismiss x is hidden (R6).

It is the same class as #2350 (Back's "Back where you were."), on the jump instead of the return. The landed row's "Linked message" bubble already marks the target, so the toast adds nothing a sighted reader needs. navigation.ts also has the card twin, "Here is the linked message's card.".

# Design

The liaison's choice is the same treatment as #2350. Both sayings join the idle status words (web/src/view/shell/notices.ts `IDLE_STATUS_WORDS`). Screen readers hear them through the polite live region, which #2350's first commit keeps in the accessibility tree, and nothing is drawn. Nothing else changes: the jump, its landing, the Linked bubble and the Back spot stay as they are.

The change stacks on #2350, since both edit the same `IDLE_STATUS_WORDS` lines.

# Acceptance Criteria

- After a summary-citation jump, on phone and desk, light and dark, with Discuss open, no toast stands over the landed row or runs under the Discuss panel. The words are said as an idle word (in the polite live region, not hidden from the accessibility tree, drawn nowhere), recorded at the moment of saying with no clock.
- The landed row still carries its Linked bubble and stands where the jump puts it (the walk's existing landing checks).
- A view test marks both sayings idle.
- Screenshots of the landing frame, before and after, each folder with its BUILD.txt.

# Notes

[claude-opus-5-5, lane cl-pagenits] Filed at the coordinator's request from the #2350 design review (2026-11-27). The liaison asked for the same idle treatment as #2350. Related: #2350, #1859 / #1862 (the idle status words), #2261 (the summary-link walk), #2270 (the Linked bubble on Back).

## Resolution

[claude-opus-5-5, lane cl-pagenits] Landed in ef14b7df (2026-11-27 6:18 AM ET, liaison LAND NOW, after #2350 landed; owner gate on 8ff3f246 fully green, 238/238 steps). "Here is the linked message." and its card twin join the idle status words, so a link jump's landing is said through the polite live region and drawn nowhere: no toast over the landed row on a phone, nothing cut under Discuss on a desk; the Linked bubble marks the target as before. tests/js/view/shell-notices.test.mjs marks both idle; tests/summary_link_browser.py records the status line the moment the jump says its landing (idle, announced, clipped). The design review's pixel diff of the landing frames is exactly the old toast box.
