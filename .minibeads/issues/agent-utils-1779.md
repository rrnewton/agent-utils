---
title: 'vibe-talk-toast-avoids-discuss-controls: dealt-with toast must not cover the docked Discuss pane controls'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- ui
created_at: 2026-11-17T12:12:16-05:00
updated_at: 2026-11-17T12:12:16-05:00
---

# Description

[watermelon-full-preview] Liaison review of the deployed batch (2026-11-17): the "N message(s) marked as dealt with here..." toast (#1711 raised allowance, the #status-line pill) sits over the docked Discuss pane's disclosure row on phone dark, keyboard open (screenshot interact-1725-undo-discuss-dark-kbdopen.png in the batch matrix). It is click-through (pointer-events: none) but visually covers a control. Keep the toast clear of the docked Discuss pane's controls, or place it in the pane's footer.

Same class, from coordb (2026-11-17, /tmp/mu-discusstweak-shots/phone-dark-resummarizing.png): while resummarizing, the same #status-line pill carrying "fetching the channel..." covers the docked pane's "Earlier summary" disclosure row. The rule is for the pill itself, whatever words it carries — the dealt-with toast and the fetching status are one element (`setStatus`).

# Acceptance Criteria

- On phone (412x915, scale 2.6) with the Discuss half-pane docked open, in light and dark, keyboard open and closed, the toast pill's bounding box does not intersect the Discuss pane (its head, disclosure rows, or ask bar) and does not cover its controls. The same holds for the pill carrying "fetching the channel..." in the resummarizing state, over the pane's "Earlier summary" disclosure row.
- The toast still appears, is readable, and its dismiss button remains hittable; message-list behaviour without Discuss open is unchanged.
- Page tests at phone and desktop sizes cover the exact defect, in light and dark where theme-related and keyboard open where layout-related, with condition-based waits only.
- Screenshot matrix on a TEST instance (light/dark x keyboard open/closed x phone/desktop, Discuss half-pane open in at least the phone shots, before and after the targeted state), every image inspected.

# Notes

[watermelon-full-preview] Filed by mu-uinits from the liaison's review of the deployed batch, bundled with #1783 vibe-talk-review-nits-1761-1762-1767 in one lane at normal priority.
