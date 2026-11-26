---
title: 'vibe-talk-discuss-split-pane: in Discuss the current summary holds still while the timeline scrolls, with a draggable boundary and a collapsible fixed-height outline'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T17:45:00-05:00
updated_at: 2026-11-25T17:45:00-05:00
---

# Description

Owner request (2026-11-25, about 5:31 PM ET), Discuss view: split the Discuss panel into two panes, the current summary and the timeline (outline). Scrolling the timeline leaves the current summary where it is. The reader can move the boundary between the two. The timeline (outline) is a collapsible section that can be made small and kept at a constant size, so the message entry (composer) under it and the latest question and answer stay visible without the long history above them.

Today the summary, the timeline and the question/answer thread share one scroller, so a long timeline pushes the composer and the latest answer out of view, and scrolling the timeline moves the summary away.

# Design

Design note first (sent to the lead before any code), covering:
- Layout at phone 412x915 and desktop 1280x800: current summary on top in its own scroller; the timeline (outline) below in its own scroller; composer and latest answer kept visible.
- The boundary: drag by mouse and touch (pointer events, with a large enough touch target), and by keyboard (a focusable separator, arrow keys step its position) for accessibility.
- Size storage: per device (local storage), never synced to the server.
- Phone keyboard open: which pane shrinks first; the composer and the latest answer stay visible.
- Composition with Discuss's existing full-screen and half-pane modes and with the glossary/WTF panel.
- Collapsed timeline: a one-line header (title and bullet count), with its minimum and fixed heights.
- Scroll ownership (the reading-row rule of #2230): nothing moves the row being read; the summary pane never moves when the timeline scrolls or grows.

# Acceptance Criteria

- In Discuss, scrolling the timeline does not move the current summary; growth of the timeline (new bullets, older pages) does not move the summary either.
- The boundary between summary and timeline moves by mouse drag, touch drag and keyboard; its position survives a reload on the same device and is not sent to the server.
- The timeline collapses to a one-line header and expands again; collapsed or at a small fixed height, the composer and the latest question and answer stay visible at phone and desktop sizes, keyboard open and closed.
- Works in the full-screen and half-pane Discuss modes and with the glossary/WTF panel open.
- Page tests at phone and desktop sizes cover the held summary, the drag, the persisted size and the collapse (condition-based waits only).
- Screenshot matrix (light and dark, keyboard open and closed, phone and desktop, Discuss half-pane open) plus the split-pane states, each folder with a BUILD.txt naming the commit, and a dynamic run against a local server.

# Notes

Owner request 374, part A (parts B and C, the stuck older load in the All view and the earlier-summary strikethrough, are separate lanes). Related: #1349 (Discuss answer as Markdown), #2073 (New summary button), #2230 (reading row stays put).

As built (design approved by the lead and the liaison with the Latest block and a 35% default share; deltas from measurement): the Latest block is capped at 30% of the panel (at 40% a long answer next to the summary's floor left the timeline no room by default, even in a full-height docked column); the summary pane's floor is its header row or 15% of the panel by default and its header row alone once the boundary was dragged; while composing on a phone (the follow-up box focused, keyboard up) the Summarize bar and the Draft saved line step aside below the panes and the summary pane gives way from its foot (top and rows fixed), so the Latest pair keeps at least two whole text lines and the composer fits the short sheet; they come back on blur without moving a summary row (nothing above the summary ever steps aside); a new Latest pair lands on its answer's first text line (its one-line sticky header keeps the question in view, the Discuss label scrolls off above) and re-lands there when the block changes height unless the reader scrolled it; on a phone's half sheet with a Latest pair the timeline defaults to collapsed (the full sheet and the docked column keep the open 35% default) and the reader's own open or collapse wins and is kept; the timeline's first draw puts a whole row at its top (liaison review, 2026-11-25). The collapsed header names earlier summaries that arrived while collapsed and never opens itself. Code: view/catchup/split.ts (size, fold, boundary, scroll-owner helpers), panel.ts (Latest split in drawTimeline), 22-discuss.css (pane layout), device settings `discussTimelineHeight` and `discussTimelineCollapsed` (per device, never sent to the server). Tests: catchup.test.mjs split tests, tests/discuss_split_browser.py (real server, model stub only), screenshot scenes 140 to 145.
