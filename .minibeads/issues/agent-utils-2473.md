---
title: 'bridge-even-hover-resample: the bridge walk re-reads the pack triangle''s hover strength after the settle and can catch the hover gone'
status: open
priority: 3
issue_type: bug
assignee: cl-filternote
labels:
- vibe-talk
- tests
created_at: 2026-11-28T12:35:00-05:00
updated_at: 2026-11-28T12:35:00-05:00
---

# Description

`vibe-talk.check.bridge-even-browser` (`vibe-talk/tests/bridge_even_browser.py`) failed once in the full gate of the `#2442` R5 commit with "bridge-even-desk-light: the triangle under the mouse is at opacity 0.55". At a desk the walk moves the mouse onto the pack triangle, waits until the triangle's opacity reaches 0.99 (that wait passed), waits for the page to settle, then reads the opacity again and checks it once more. The second read found 0.55, the triangle's resting strength, so the hover was gone between the wait and the read. The hover is pure CSS (`:root:not([data-input="keys"]) #side-cards .reply-ungather.unpack-pack:hover .unpack-pack-mark { opacity: 1 }`).

Not reproduced locally: 13 of 13 passes on the R5 commit and 13 of 13 on main (5 alone and 8 at once each, load1 220 to 253); the gate's rerun of the step passed. R5 does not touch the side card, its styles or the input mode. So the check samples a hover state a second time under load, after the condition it waited on already held; something that drops the hover for a moment (the card redrawn under a still mouse, with the browser's re-hover after layout running late, is a guess, not traced) fails it.

# Design

- Wait on the hovered state itself and take the values from that same read: one `wait_for_function` that returns the hovered row's opacity, background and geometry once the opacity is at full strength, instead of a wait followed by a separate settle and re-sample.
- If the walk must also prove the hover holds, require it over consecutive frames inside the page (a short rAF loop) rather than at one later instant, so a lost-and-regained hover reads as a retry, not a failure; keep the check's meaning (the triangle under the mouse comes to full strength and draws its rounded rectangle).

# Acceptance Criteria

- The walk passes 20 of 20 at once (8 or more copies concurrently) on a host at load1 >= 237, and still fails if the hover rule is removed from the stylesheet (negative control).
- No wall-clock budget added; waits stay on conditions with the hang backstop.

# Notes

- Filed at the coordinator's request after the `#2442` R5 gate (number pre-assigned); low priority, after `#2453`. Evidence: the R5 gate log of `vibe-talk.check.bridge-even-browser` and the A/B runs in the R5 lane's report.
