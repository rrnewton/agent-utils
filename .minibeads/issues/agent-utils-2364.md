---
title: 'vibe-talk-dark-filled-button-contrast: white text on the dark theme filled button reads at 2.9:1'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T04:05:00-05:00
updated_at: 2026-11-27T04:05:00-05:00
---

# Description

In the dark scheme the page's filled button (`button` in web/style.css: `--accent` ground, `--on-accent` label) is white on `#5b9dd9`, about 2.9:1. That is under the 4.5:1 WCAG AA asks of text. It is the default Blue preset's dark pair, and it dresses every filled control: the channel Send, Pace and Read in the dock, and (since #2301 item 7) Discuss's Send while its composer holds the focus.

Found by the advisory design review of #2301 vibe-talk-ui-review-findings-11-26 item 7 (dark frames of scenes 73-77, phone and desk), noted outside that change.

The pair is a documented exception today, not an oversight. #1961 vibe-talk-colour-themes kept Blue's dark pair unchanged as the look the owner already had, "kept unchanged until they pick otherwise". It added the `blue-hc` preset, the same family with dark ink on the accent (6.27:1), so the two can be compared. The core settings unit test asserts 4.5:1 or better for every preset and pair except that one.

# Design

The owner has to pick first, because the exception is theirs (#1961). The options:

1. Keep the exception. The default stays white on `#5b9dd9`, and `blue-hc` stays the repaired choice in Settings. Close this issue as decided.
2. Repair the default. Blue's dark `--on-accent` becomes `blue-hc`'s dark ink (`#14161a`, 6.27:1) in COLOUR_PRESETS (web/src/core/settings/colour.ts) and in the dark block of web/style.css, so the sheets alone paint the same before any script. The core settings test then asserts AA for every preset and pair with no exception, and `blue-hc` either goes or stays as an alias.
3. Repair the ground instead: a darker dark accent under white. This changes every accent use (links, rings, chips), not only filled buttons, so it is the widest of the three.

Whichever is chosen, nothing in the layout changes: only colour tokens.

# Acceptance Criteria

- The owner's choice is recorded here.
- If the default is repaired: every filled button's label reads at 4.5:1 or better in both schemes for every preset. The core settings test asserts it with no exception, and a browser check measures the rendered label on the channel Send in dark.
- Screenshot matrix (light and dark, phone and desk, keyboard open and closed, Discuss open; R16) before and after, each folder with its BUILD.txt.

# Notes

[claude-opus-5-5, lane cl-pagenits] Filed at the coordinator's request from the #2301 item 7 design review (2026-11-27), in the page-nits list, separate from the Pace/Read question (#2301 item 8). Related: #1961 vibe-talk-colour-themes (the documented exception and `blue-hc`), #1899 (the search bucket chip's own per-scheme AA pair).
