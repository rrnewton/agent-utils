---
title: 'mention-jump-in-view: a mention jump lands its highlighted message in view and keeps it there'
status: closed
priority: 0
issue_type: bug
created_at: 2026-11-10T16:34:00+00:00
updated_at: 2026-11-11T03:20:40.902680250+00:00
closed_at: 2026-11-11T03:20:40.902680120+00:00
---

# Description

[watermelon-full-preview] mu-docjump. Owner report (reply 312, 2026-11-10 11:11 AM ET), verbatim in substance: clicking a mention in the Documents list takes them to a page with the message, it does the prominent highlight of "Linked message", but it ends up being off the screen. "This is one more thing with our busted scroll control."

The mention jump comes from #1047 documents-list-order (open: "mention jump with Back"). The highlight comes from #890 voice-page-linked-highlight (closed). No existing issue covers the jump landing off screen; the tracker was searched before filing.

# Design

Page-only fix in the vibe-talk page, through the existing scroll anchor and keeper path, not a one-off scroll jump that later corrections undo:

- On a mention jump (and any linked or highlighted jump of the same shape), the jump target becomes the scroll anchor. The highlighted message lands in view and stays there while late content arrives above it: live arrivals, summaries turning on or off, reactions, the freshness line, and history paging above.
- Go through the page's existing anchor and keeper path (preservingScroll and the list keepers) so later corrections keep the target instead of undoing it. Coordinate before editing shared scroll-control ground: the chip observers and keepers are mu-scroll's (#1209 selection-survives-updates, #1212), and the single-scroll-owner work is mu-stable's (#1210 single scroll owner). If the fix needs their ground, sequence it through the coordinator instead of editing under them.
- Owner reply 298: nothing the user is looking at moves unless the user acted. The jump is a user act, so moving to the target is intended; background arrivals after it must not move the target out of view.
- Companion trace (add-on from the coordinator, owner reply 312): #1208 auto-sent scroll traces only fire when the view moves without a user act, and a click counts as one, so this jump sent no trace when it was lost. Add a small trigger, coordinated with mu-scroll who owns #1208: when a jump target (linked or highlighted message) leaves the viewport within about 10 seconds of the jump, send the trace with kind=jump-lost through the existing trace send path. Ship it in the same gate as the fix.
- Do not move code for #624 voice-page-modularity; keep the edit small and rebase onto moved code if the touched lines move.

# Acceptance Criteria

- Clicking a mention in the Documents list lands the highlighted ("Linked message") row inside the viewport at phone size (393x852) and at desktop size, and it is still inside the viewport after the page settles.
- Settling includes live arrivals, summaries off and on, reactions, the freshness line, and history paging above; none of them push the highlighted row off screen.
- A browser test at both sizes clicks a mention in the Documents list, waits for the page to settle under those late arrivals, then asserts the highlighted row is inside the viewport and stays there.
- When a jump target leaves the viewport within about 10 seconds of the jump, a scroll trace with kind=jump-lost is sent; a test asserts the trace fires when the target is pushed off screen.
- The fix goes through the existing anchor and keeper path (preservingScroll and the list keepers); no one-off scroll call that later corrections can undo.
- Validation is the owner-tier path-selected gate (page-only). If the change touches the server, a full follow-up gate is named in the landing note.

# Notes

Owner reply 312, 2026-11-10. Slot docjump, branch mu-docjump/mention-jump-in-view. Related: #1047 documents-list-order, #890 voice-page-linked-highlight, #1208 scroll-trace-layout-shift, #1209 selection-survives-updates, #1210 ui-stability-no-async-shift, #1212, #624 voice-page-modularity.
