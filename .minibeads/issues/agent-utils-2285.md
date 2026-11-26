---
title: 'vibe-talk-labels-browser-tapped-message-jumps: a tapped message moves 43 px in labels_browser.py on main'
status: open
priority: 0
issue_type: bug
assignee: cl-tapjump
labels:
- vibe-talk
- owner-ui
- scroll-stability
created_at: 2026-11-26T05:25:00-05:00
updated_at: 2026-11-26T05:25:00-05:00
---

# Description

`vibe-talk/tests/labels_browser.py` (a browser walk on a fake API; not in the validation DAG) fails on main 1a19cf54 at step 4, "A chip tap filters, and the tapped message keeps its screen place": "the tapped message jumped: 206 -> 163". Tapping a label chip on a message opens the `label:<slug>` search, and the tapped message's top moves up by 43 px. The check allows the row to move down by the search bar's height at most and never up, so a move up is a real jump of the row being read.

This is the class of defect owner rule 370 forbids: the row being read never moves, and growth extends below it. Found by the #2279 vibe-talk-post-facto-labels lane while it ran the walk on main.

Triage questions this issue answers:

- Is it deterministic or flaky (three runs on current main, load1 recorded per run)?
- Is it owner-visible on the deployed build (the walk run against the deployed SHA, on a local server or the walk's own fake API only)?
- Which landing caused it (bisect over the page and the walk between a known-good point and main), or is the walk's expectation stale?

# Design

Triage first; the cause and the proposed fix are recorded here once the lead approves them.

# Acceptance Criteria

- The cause commit is named with bisect evidence, or the walk's expectation is shown to be stale with evidence.
- After the fix, tapping a label chip keeps the tapped message where it was on the screen (per-frame trace of its top within 0.5 px, or 1.0 px where placement is on whole pixels), on phone 412x915 and desktop 1280x800.
- A page test at phone and desktop sizes pins the exact defect, with condition-based waits only.
- A proposal for putting `labels_browser.py`, or just its tap check, into the validation DAG cheaply (fast, high-signal, condition-based, robust at load1 >= 237).
- Screenshot matrix (light and dark, keyboard open and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in the phone shots) plus before and after frames of the chip tap, each folder with its own BUILD.txt.

# Notes

Related: #1825 (hashtag labels, chips on cards), #1853 (Labels drop-down), #243 (the place-keeper the check relies on), #2279 (the lane that found the failure).
