---
title: 'draft-return-thread-list: back from Settings with a new draft, the thread list reader is left above the foot'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T15:52:38.645914593+00:00
updated_at: 2026-10-19T10:29:46.955818655+00:00
closed_at: 2026-10-19T10:29:46.955818514+00:00
---

# Description

Found in the final review of #269 approval-card-inline (2026-10-15): a reader at the foot of the thread list's cards who gets a voice-agent draft while on Settings or Help comes back with the send card docked under the cards and is left about 247px above their foot. Send stays in reach, so it is not blocking, but nothing should move under a reader who did not move (#280). When the draft arrives while they are on the list, they stay at the foot. Fix the return path to place the reader as the arrive-while-on-the-list path does; Chromium check at 393x852 and 1280x800.
