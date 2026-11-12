---
title: 'discuss-panel-resize: wider default overlay, drag-resizable, phone half-height sheet'
status: closed
priority: 0
issue_type: feature
assignee: mu-discusslay
labels:
- vibe-talk
created_at: 2026-11-11T19:01:00+00:00
updated_at: 2026-11-12T11:30:00+00:00
---

# Description

Owner reply 413 (2026-11-11): the Discuss panel "pops up as a narrow column on the right hand side of my window. It's too narrow. It should be resizable for if I need more content." Owner reply 417 (via coordb): on mobile the panel is "so big I think it's confusing to see just a little of the text and decorations poking through from around the edges... I think it should be FULL width but only HALF height showing some of the messages above it. But have a maximize button to take it the rest of the way to full height. And when it's state is toggled from half to full height it should stick. Close and open should leave it right back where it was."

Lane mu-discusslay owns the Discuss panel's LAYOUT (mu-catchup owns content/behaviour, mu-discussq the prompt). This issue is the first layout landing; `#1354 discuss-layout-modes` (overlay/docked/maximized) follows on its own.

# Design

- **Desk overlay.** Wider default (560px, was 24rem), drag-resizable by the panel's left edge: a `role=separator` grip with pointer drag (kept once, at the end of the drag), arrow/Page keys, Home/End, and double-click reset — the two-column splitter's shape (`#1186`). The width is written as `--discuss-width`, which the stylesheet alone decides applies (desk media only), and resizing never reflows the list or writes a scroll position: the panel floats outside `#scroll-area` (#1251 owns scroll; no reader place needs keeping).
- **Per-device keep.** Width in `DEVICE_SETTINGS.discussWidth` (`vibe-talk.voice.discuss-width`, clamped 320–960, default 560) over the platform device-storage port (localStorage), handed to the view as deps — the page's home for similar per-device layout state, as the two-column split is kept. Synced settings (server LWW, cross-device) are the wrong home and are untouched.
- **Phone sheet.** Full-width bottom sheet (edge to edge, no peek-around-edges) at half height (`50dvh`) by default with messages visible above; the header's Maximize/Restore toggles full height. The choice is `DEVICE_SETTINGS.discussSheetFull`, so it sticks across close/reopen (and beside the `#1345` transcript once that lands). The stylesheet decides the sheet applies (non-desk media); the panel only sets `data-sheet`.
- **Header slot.** The header gains a named actions slot (`#discuss-actions`: sheet toggle, then Close) so `#1346`'s Summarize button and `#1354`'s mode control join beside Close without moving it. `voice.js` gains only layout thunks at the existing `createDiscussView` construction (cleared with mu-catchup); the `#discuss-open` phone bar rule is left as landed for `#1355 phone-bar-discuss-slot`.

# Acceptance Criteria

- Desktop 1280x800: Discuss opens at the wider 560px default; dragging its left edge widens/narrows it live and the width is kept per device (survives close/reopen); the grip's keys resize and double-click restores the default; opening and resizing never move `#scroll-area`.
- Phone 390x844 and 393x852: Discuss is a full-width sheet at half height with messages visible above; Maximize stands it at full height, the choice persists across close/reopen, Restore returns it to half.
- Unit tests pin the default, the clamp, drag-keeps-once, the keys, and sheet persistence; the Discuss browser walk asserts all of the above in Chromium; page invariants and the owner-tier gate pass.

# Notes

Owner replies 413 and 415 (overlay resizable; dock/maximize deferred to #1354), 417 (phone half-height sheet). Split agreed with mu-catchup (header actions slot for #1346; no panel.ts/CSS contention while #1348 stays server-only) and mu-phonebar (#1355 owns the #discuss-open phone rule; entry path reused unchanged). Landed by mu-discusslay; coordb deploys.
