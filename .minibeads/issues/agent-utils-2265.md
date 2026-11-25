---
title: 'vibe-talk-summary-bullet-restyle-everywhere: Done or archive on a cited message restyles every bullet that cites it, Earlier summaries included'
status: open
priority: 0
issue_type: bug
assignee: cl-bulletstrike
labels:
- vibe-talk
created_at: 2026-11-25T22:40:00.000000000+00:00
updated_at: 2026-11-25T22:40:00.000000000+00:00
---

# Description

Owner request (2026-11-25, part C of a three-part Discuss request; the pane split and the All view's older-message loading are separate issues), verbatim: "when I clicked the following message after reload it DID go back far enough for that message: [a citation link to an older message in the All view]. But when I archived it I didn't get strikethrough on the bullet on the OLD 'Earlier Summary'. If it's not too much overhead I would like it to apply there. I.e. maybe any message linked to a summary bullet pushes an event which makes a style change to that bullet even if its in earlier summary."

Today the live strike (#1388 discuss-summary-staleness-update, #1820 vibe-talk-summary-strikethrough-live) restates only the current summary: the panel's `restrike` walks every bullet and section under the drawn answer on each mark. The Earlier-summary bodies the Discuss timeline draws are never struck, so archiving or marking Done a message one of them cites leaves that bullet plain, even while the same message's bullet in the current summary strikes through. Each restate also rescans every bullet of the current answer, whatever message changed.

# Design

Event-driven and keyed by message id. A per-panel strike index maps each cited message id to the bullet and section elements that cite it (each entry keeps its own cited ids and its group). Drawing the current answer and drawing the timeline each replace their group's entries from the freshly drawn tree, so a redraw, a restored record or an Update never leaves stale or detached elements indexed, and strike just that group.

A state change that knows its ids (Done, archive, put back, a refused write's take-back, the Discuss panel's own mark buttons) passes them through `onReadStateChanged` and the page's strike refresh; the panel does one index lookup per id and recomputes only the entries found. Changes without an id list (server read state landing, view sync, the settle wrappers) diff the index's own id keys against a cached read bit and touch only entries whose cited id flipped; no DOM walk per event. An upstream delete restates through the same lookup with the current read rule. The current summary's fresh (green) marking keeps its present behaviour.

# Acceptance Criteria

- Archiving or marking Done a message cited by a bullet in the current summary AND by a bullet in an Earlier summary strikes both bullets at once, with no refresh; putting it back clears both.
- A bullet is struck only while all its cited messages are read; an uncited bullet is never struck.
- Redrawing the current summary or the timeline leaves no stale index entries; a strike update after a redraw reaches the new elements.
- A state change looks up only the bullets citing the changed ids; no scan of all bullets per event.
- A page test that fails on main proves the Earlier-summary strike and its clear, at phone and desktop sizes, through the real server router.
- Screenshot matrix on a test instance (light and dark, keyboard open and closed, phone 412x915 and desktop 1280x800, Discuss open) plus the struck Earlier-summary bullet before and after.

# Notes

Owner fast-track (owner-priority gate, deployed on landing). Siblings from the same owner request: the Discuss pane split with a collapsible timeline, and the All view's stale timeline and failing Loading older.
