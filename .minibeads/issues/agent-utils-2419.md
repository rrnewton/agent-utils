---
title: 'vibe-talk-read-run-separator-over-label: the folded read run line draws its rule through "N read messages, M pinned", and the label is not centred'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T19:04:07-05:00
updated_at: 2026-11-27T23:09:06-05:00
closed_at: 2026-11-27T23:09:06-05:00
---

# Description

The owner, 2026-11-27 7:01 PM ET, verbatim: "Corrupt rendering of separator line . "23 read messages" not centered.. looks like separator line is rendered OVER the text labels."

From the owner's screenshot (the liaison's description; desktop, dark, a channel list):
- The folded read run's line reads "First 5 ———— [… 23 read messages, 2 pinned … ] Last 5".
- The middle label, a dashed box, is pushed to the right instead of centred.
- The horizontal rule runs straight through the "23 read messages" text, like a strike-through.

The pinned run's line is drawn in pieces (#1854): the head "… N read messages,", the separate "M pinned" target, and the tail. The First/Last peel buttons sit at its ends (view/channel/readrunlines.ts). Either the rule is not broken around the label, or the label pieces lack the opaque background that hides it, and the group is not centred.

# Design

- Find the cause by bisecting the recent commits that touched the run line and its seam/fold marks, and check the live build (2eebd8ea).
- Reproduce locally with a fixture: a folded read run WITH pinned items, desk and phone, light and dark.
- The rule stops at the label's box on both sides (or the label covers it opaquely), and the label group stands centred between the peel buttons.

# Acceptance Criteria

- A browser walk on a folded read run with pinned items, at desk and phone in light and dark. It checks by geometry that the label group is centred between the rules (or the peel buttons) within 1 px, and that no part of the rule crosses the label's box. Main's page fails it.
- The plain run (no pinned items) keeps its look.
- Owner-UI evidence: phone and desk, light and dark, keyboard open and closed, Discuss open, before and after, with BUILD.txt.

# Notes

Filed at the coordinator's request (owner-UI fast track), issue file only.

## Resolution

Fixed by 9eeaa847 (vibe-talk: a folded run's line with pins keeps its rules in the line, whatever its head row's state). Deployed; post-deploy guards green (read_run_line, fold_pins, read_modes).

- Cause: a pinned run's line draws its two dashed rules as the head row's own ::before and ::after. Row states (the pin edge, the read veil of a reply / seam / bookmark / thread strip, the landed and Done moments) also claim li::after and make it position: absolute.
- Effect: with a pinned or reply head, or the line selected, the right rule left the line. The sentence stood far right, and the veil drew the rule across the whole row through the words: the owner's screenshot.
- Fix: both rules reset to plain flex items (position static, no inset, background, shadow, outline or animation, !important).
- Walk: tests/read_run_line_browser.py (dag 190), 18 cases including the owner's view at 1117x800. Main failed every pinned, reply and selected head.
- The line stays at list width (liaison 1095).
