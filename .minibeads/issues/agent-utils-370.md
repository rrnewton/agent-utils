---
title: 'shift-e-undo-marker: Shift+E asks the chat service even when its own Done was undone while saving'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T12:15:02.803941364+00:00
updated_at: 2026-10-19T12:18:29.178642351+00:00
closed_at: 2026-10-19T12:18:29.178642181+00:00
---

# Description

Found in the review of #327 done-races-remaining (2026-10-19); the same race as #327 part (3), on main before that lane. Shift+E (readAndCollapse) on a message not Done yet marks it Done and then asks the chat service to move its read marker without looking again. An Undo pressed while Shift+E's own Done is being saved, with the Undo itself saved, still moves the marker, for a message the server no longer holds Done. The chat service's marker cannot be moved back. After its own Done, Shift+E must look again the way a sent reply does since #327 (doneSaved), and ask nothing once the message was put back meanwhile. Page test with a negative control.
