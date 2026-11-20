---
title: 'vibe-talk-windowed-all-view-fixes: windowed All view thread chips, fold anchor, END and Newest'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- scroll
- performance
created_at: 2026-11-20T14:20:00-05:00
updated_at: 2026-11-20T14:20:00-05:00
---

# Description

[watermelon-full-preview] Owner bug 262 (2026-11-20 2:17 PM ET), windowed lists ON, All view: (a) thread chips show a bare Thread label for about a minute before slug and count fill in; (b) the semicolon fold toggle jumps far back and does not keep the anchor; (c) END does not land at the bottom and jumps around; (d) the Newest button does nothing. The owner has turned windowed lists back OFF. Fixing this unblocks moving windowed lists towards the default.

# Design

- Reproduce on a local fixture with windowed lists ON in the All view (several threads, enough rows that windowing kicks in), at the end of scrollback and mid-history; never call the live server.
- Invariants as gate checks (event counts, never wall time): thread chips show slug and count on first render (or within one render pass after data arrives); the semicolon fold keeps the anchor message within a few px; END lands at the bottom row; Newest scrolls to the newest row; all with windowed ON and also OFF. End-to-end through the real router.
- Coordinate with the standing perf lane (windowed render path, perf budget) and the All-view send-flash lane (default path).

# Acceptance Criteria

- All four defects fixed in the windowed All view, with the windowed-OFF path behaving as before.
- Focused page tests at phone and desktop sizes for the exact defects; dynamic exercise on a local test instance through the real server router before landing, with evidence.
- One owner-priority gate on the final SHA, then land to main directly per this repository's publication policy.

# Notes

[watermelon-full-preview] Number pre-assigned by the lead coordinator for this lane. Related: #1846 vibe-talk-windowed-rendering (windowed steps 1-6 landed, S2 default-on still pending), #1983 All-view send flash (default path).
