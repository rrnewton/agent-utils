---
title: 'discuss-summarize-bottom-bar-mobile: Summarize control as a bottom bar on mobile'
status: open
priority: 1
issue_type: task
created_at: 2026-11-14T15:49:00.000000000+00:00
updated_at: 2026-11-14T15:49:00.000000000+00:00
---

# Description

Owner UI request 056 (2026-11-14): on mobile the Summarize control should be a control bar at the bottom of the Discuss pane, closer to the thumb, with the Summarize button at the end of the line ("Last K .... [Summarize]") so it is easy to reach right-handed. On desktop the Discuss header stays exactly as it is.

# Design

Placement in `vibe-talk/web/styles/voice/22-discuss.css` plus a one-line DOM move in `voice.html`: `.discuss-ask` stands as a direct child of `#discuss-panel` right after `.discuss-head` (its contents, including the `.discuss-bound` span owned by #1524, untouched), because a sticky bar cannot escape a short head parent. On the phone regime `not ((min-width: 900px) and (pointer: fine))` the pane becomes a column flex; the ask takes the last flex order, sticks to the pane's bottom edge (`position: sticky` with `margin-top: auto`), and spans the pane's full width as a bar with the pane's own ground and a top rule. Inside it, `.discuss-bound` keeps the start and `#discuss-summarize` takes the end (`order` plus `margin-left: auto`), so the line reads "[N] unread, [K] hours [since HH:MM] [Summarize]" on one row. The pane keeps `overflow-y: auto` and its half/full heights, so the docked half-height pane from #1526 and the sticky top bar from #1531 compose unchanged, and the stuck bar sits at the pane's bottom, above the page's bottom toolbar and clear of the on-screen keyboard (the pane's `dvh` height tracks it). On desktop the ask keeps its one-row place beside the actions until #1531's title bar stands in the head, then flows under it.

# Acceptance Criteria

- At phone width (~390 px) the ask bar is at the Discuss pane's bottom, the Summarize button is the right-most control and fully visible, and the final bound content plus the button hold one row.
- At desktop width the Discuss layout is unchanged (ask row at the pane top).
- No change to `vibe-talk/web/voice.js` or `web/src/app/main.js`; no change inside `.discuss-bound`.
- Browser coverage in `vibe-talk/tests/discuss_browser.py` asserts the phone bottom bar and the desktop top row with condition waits only.

# Notes

Owner request 056; sibling lanes #1524 (K-hours bound), #1525 (heading fold), #1526 (mobile docked Discuss) land independently; whoever lands second rebases and re-gates.
