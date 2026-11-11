---
title: 'collapse-anchor-below: collapsing a message whose top is off screen keeps the next message fixed'
status: open
priority: 0
issue_type: feature
assignee: mu-renderloop
created_at: 2026-11-10T15:05:09.000000000+00:00
updated_at: 2026-11-11T16:09:08+00:00
---

# Description

Owner request (reply 307, 2026-11-10), a nice-to-have that follows #1210 ui-stability-no-async-shift. Parked until the stability work lands.

Today, collapsing (folding) a message keeps that message's top fixed on screen, and the history below moves up to fill the gap. That is right while the reader is focusing on the message being collapsed, with its top in view. But often the reader collapses message N precisely to move on to message N+1: they have already scrolled down to focus on N+1, the top of N is already off the screen, and they tap near the bottom of N to shrink it. Holding N's (off-screen) top fixed then throws N+1 up the screen — a long collapse can throw it completely off the screen, which is disorienting. "If the point we would hold fixed on the screen is already off the screen then there is no benefit in holding it fixed."

Rule: when collapsing message N, if N's top is on screen, keep N's top fixed (today's behaviour); if N's top is above the viewport, keep message N+1 fixed on screen and let the history above absorb the change.

Owner reply 380 (2026-11-11): unparked now that #1229 and #1251's scroll owner exist, at owner tier, and it adds a third case: "If we're in a corner case where we have a huge message that exceeds the screen's vertical height and we are in the MIDDLE of that message... maybe we can put the giant message we just collapsed CENTERED on the screen vertically."

Rule, all three cases:
- (a) N's top is on screen: keep N's top fixed.
- (b) N's top is above the screen and N+1 is on screen: keep N+1 fixed.
- (c) Neither is on screen (mid a giant message): center the collapsed N vertically.

# Design

Part of the one-scroll-owner design under #1210: the anchor choice for a collapse. The single owner of the scroll position picks its anchor per act — for a fold/collapse, the collapsed row's top while that top is inside the viewport, otherwise the first row below it (N+1, or the first on-screen row at or below the collapse). The same choice applies symmetrically when expanding. Page-only, in the view/channel modules that own folding and scroll keeping; no server change.

# Acceptance Criteria

- Collapsing a message whose top is on screen keeps that top where it was, as today.
- Collapsing a message whose top is above the viewport keeps the next message where it was on screen; the history above absorbs the height change.
- Collapsing while neither N's top nor N+1 is on screen (inside a message taller than the viewport) centers the collapsed N vertically.
- A browser test covers all three cases (tap near the top with the top visible; tap near the bottom with the top off screen), at phone and desktop widths.
- The stability soak under #1210 stays green (or report-only values do not regress) with the change.

# Notes

Owner reply 307, 2026-11-10 10:00 AM ET. Follow-up to #1210 ui-stability-no-async-shift; the tracker was searched and no existing issue covers it. Parked until the stability work lands. Mentioned in the redraw-model design note as the collapse anchor choice for the one scroll owner.
