---
title: 'refused-undo-restores-done: a refused Undo puts back a Done the server also refused'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T13:11:36.506972276+00:00
updated_at: 2026-10-15T13:11:36.506972276+00:00
---

# Description

Found in the final reviews of #306 done-stale-across-views / #311 sent-read-waits-for-done (2026-10-15); behaves the same on main before those changes. A Done is pressed on a message, then Undo while the Done is still being saved, and the server refuses both. Taking back the refused Undo puts back the Done the server already refused, so every view draws the message Done while the server holds none; a later write saves it into the snapshot, and a Reply on it then writes no Done but asks the chat service to mark it read (after_reply), saying 'Marked read here and in <provider>'. The chat service's marker cannot be moved back. Taking back a refused Undo must restore the state the server holds (not Done when the Done was refused), and a reply must never ask the chat service on the strength of a Done the server refused. Page tests with negative controls for refuse/refuse, refuse/save and save/refuse orders.
