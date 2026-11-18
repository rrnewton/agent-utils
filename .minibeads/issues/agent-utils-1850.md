---
title: 'vibe-talk-fold-separator-shows-pinned: split pinned messages out of the fold separator'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
- ui
- pins
created_at: 2026-11-18T10:35:00-05:00
updated_at: 2026-11-18T12:10:00-05:00
---

# Description

[watermelon-full-preview] Owner request (thread 189): split out pinned messages in the "… N read messages …" separator from Fold read. A fold that contains pinned messages shows "… N read messages, M pinned …". Tapping "M pinned" expands ONLY the pinned rows in place, splitting the fold into collapsed separators before and after them (counts updated on each piece); tapping the rest of the separator expands the whole fold as today. Builds on today's pins feature; does not depend on Pinned becoming a built-in label.

# Design

- The run plan (`createReadRuns` in present/rows) counts pinned messages per folded run from the held pins list, and the separator line renders the pinned clause only when the count is above zero.
- The separator carries two distinct targets: the main line (expand whole fold, as today) and a separate "M pinned" button (expand only pinned rows). Revealing pinned rows reuses the peel mechanism: pinned ids join the peeled set, so the plan splits the run into folded segments before and after them, each with its own updated count.
- Keyboard and screen-reader accessible: two focusable buttons with distinct aria-labels ("Show N read messages" / "Show M pinned messages in this folded run").
- Live updates: a pin or unpin inside a fold re-runs the run placement in place (line text and counts updated on the existing rows), without rebuilding the row elements; coordinate with #1839 vibe-talk-composer-stays-in-view-while-dictating row reuse.

# Acceptance Criteria

- A fold containing pinned messages reads "… N read messages, M pinned …"; a fold without pins reads exactly as before.
- Tapping "M pinned" reveals only the pinned rows, leaving collapsed separators before and after with correct counts; tapping the rest of the separator expands the whole fold as today.
- A pin or unpin inside a folded run updates its count incrementally without replacing the row elements.
- Deterministic page/browser tests cover the split at phone and desktop sizes; an end-to-end test drives the real page against the real server routes (pins served by the real pins route) on a test instance.
- Screenshot matrix (light/dark, keyboard open/closed, phone/desktop, Discuss half-pane on phone, before/after) captured on a test instance and inspected.

# Notes

Owner thread 189 (2026-11-18), via coordb lane mu-foldpins. Proof per standing dynamic-testing rule (owner thread 185): real page against real server routes, deterministic browser test, shots of a fold with a pin before/after tapping "M pinned" and after tapping the rest, phone and desk, light and dark, keyboard open in one phone shot.

## Resolution

Landed on main as 1990702f ("vibe-talk: name pinned messages in the fold
separator and open them alone"), gated at 2d3d022d (gate ledger pass,
2026-11-18T17:05:14Z, owner priority). Folded runs holding pins read
"… N read messages, M pinned …" with two labelled targets; the pinned
target reveals only the pinned rows between updated separators, and pin
changes update the line in place on the existing rows. Proof: page test
in tests/js/page/reading-state.mjs, unit wording in
tests/js/present/rows.test.mjs, real-Chromium walks in
tests/fold_pins_browser.py (phone/desk, light/dark, keyboard shrunk,
Discuss open on phone), screenshots inspected in /tmp/foldpins-shots.
Deployed on landing per owner-priority gate; coordb deploys.
