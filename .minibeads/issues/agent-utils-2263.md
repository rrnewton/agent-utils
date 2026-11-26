---
title: 'vibe-talk-discuss-split-pane: in Discuss the current summary holds still while the timeline scrolls, with a draggable boundary and a collapsible fixed-height outline'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T17:45:00-05:00
updated_at: 2026-11-26T01:04:00-05:00
closed_at: 2026-11-26T01:04:00-05:00
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

As built (design approved by the lead and the liaison with the Latest block and a 35% default share; deltas from measurement): the Latest block is capped at 30% of the panel (at 40% a long answer next to the summary's floor left the timeline no room by default, even in a full-height docked column); the summary pane's floor is its header row or 15% of the panel by default and its header row alone once the boundary was dragged; while composing on a phone (the follow-up box focused, keyboard up) the Summarize bar and the Draft saved line step aside below the panes and the summary pane gives way from its foot (top and rows fixed), so the Latest pair keeps at least two whole text lines and the composer fits the short sheet; they come back on blur without moving a summary row (nothing above the summary ever steps aside); a new Latest pair lands on its answer's first text line (its one-line sticky header keeps the question in view, the Discuss label scrolls off above) and re-lands there when the block changes height unless the reader scrolled it; on a phone's half sheet with a Latest pair the timeline defaults to collapsed (the full sheet and the docked column keep the open 35% default) and the reader's own open or collapse wins and is kept; the timeline's first draw puts a whole row at its top (liaison review, 2026-11-25). The collapsed header names earlier summaries that arrived while collapsed and never opens itself. Code: view/catchup/split.ts (size, fold, boundary, scroll-owner helpers), panel.ts (Latest split in drawTimeline), 22-discuss.css (pane layout), device settings `discussTimelineHeight` and `discussTimelineCollapsed` (per device, never sent to the server). Tests: catchup.test.mjs split tests, tests/discuss_split_browser.py (real server, model stub only), screenshot scenes 140 to 146. The half-sheet default reads the regime the stylesheet names (`--discuss-half-sheet`), never a script media query.

## Resolution

[opus 5.5] Landed on github/main as `a7f18389` (nine commits, `810622c9` through `a7f18389`), replayed by the coordinator from the gated head `fdf166f9` (on `0ab352c1`; main had gained only an issue file since). The owner gate (path-selected validate, 226 steps) passed with no failed or known-red steps, 12:28 to 12:44 AM ET on 2026-11-26. The change was deployed on landing and the watch was clean (live since about 12:47 AM ET; startup probe passed, no app errors, panics or restarts, both services active). What landed: the Discuss panel is a column of its own scrollers. The current summary pane holds still while the timeline scrolls or grows (rows dy 0). The timeline keeps a draggable boundary (mouse, touch, keyboard separator, double-click resets) at a per-device kept height, and it collapses to a one-line header with its count and a hint for earlier summaries that arrived while collapsed. The Latest pair sits above the composer, and with the keyboard up at least two whole body lines of it stay visible. All the as-built points above are included. Evidence, reviewed by the liaison before landing: the keyboard-up pass (light and dark, phone 412x915 and desktop 1280x800, keyboard open and closed, half and full sheet, docked column) and the rebased-tree carry-over. The pixel diff of scenes 74, 75, 140, 143 and 146 against the accepted shots differed only in clock labels and in the channel behind the sheet (run order); 73 and 76 were reviewed by eye. tests/discuss_split_browser.py passed in three corners (half sheet 4 whole Latest lines, keyboard up 2), along with the seven existing Discuss browser checks, the page suites and the unit step, all on a local server with only the model stubbed. Two defects were found before the gate and never landed. First, a message-only reword had re-parented the branch onto a later main commit, which would have reverted it; the chain was rebuilt on its true base. Second, the half-sheet default used a script media query, which the page suite forbids (the layout regime is the stylesheet's alone); the stylesheet now names the regime in `--discuss-half-sheet`. This also answers the phone 74/75 report of the next summary heading clipped under the Ask box: the composer now stands below the panes, and each pane clips at its own scroll edge.
