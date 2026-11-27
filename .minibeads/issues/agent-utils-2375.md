---
title: 'vibe-talk-reading-off-toast-covers-sent-row: the "reading mode off." toast floats over the header of the row the reader just sent'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T07:28:00-05:00
updated_at: 2026-11-27T07:28:00-05:00
---

# Description

Seen in the #2367 vibe-talk-read-row-moves-start-stop evidence. A read was stopped at 393x852, then the reader sent a message from the channel composer. The "reading mode off." toast floats over the newly sent row's header (its time and Unarchive control), dark and light, on main 39c0d887 and with #2367 alike. It is the same class as #2350 vibe-talk-back-toast-covers-returned-row and #2365 vibe-talk-jump-toast-covers-landed-row: a transient notice drawn over the very row the reader's act just put on screen.

# Design

- As #2365 settled: announce it, don't float it. The notice that reading mode went off is spoken to assistive technology and shown where it covers no row (or not drawn over the list at all), so the sent row stands clear.

# Acceptance Criteria

- Shots at 393x852 and 412x915, light and dark, of a read stopped and then a message sent: the sent row's header fully visible, no toast over any row, with BUILD.txt.
- The notice stays available to a screen reader (an announcement), checked by the page test or the browser check that covers #2365's announcement.

# Notes

Filed at the coordinator's request from #2367's evidence (issue file only). A morning-batch candidate; no code yet. Related: #2350, #2365, #2367.
