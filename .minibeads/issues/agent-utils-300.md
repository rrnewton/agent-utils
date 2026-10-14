---
title: 'reply-marks-read: a reply marks read consistently, whether started from Reply or from the thread'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-14T16:11:59.821144823+00:00
updated_at: 2026-10-14T17:08:22.448270377+00:00
closed_at: 2026-10-14T17:08:22.448270127+00:00
---

# Description

Owner 2026-10-14: Reply on a message and opening its thread to write both create a reply to the prior message, but they mark read differently. Rule agreed: (1) the first reply that starts a new thread marks the root read, whichever way it was started (Reply button, or the thread opened and written in); (2) later messages written in an existing thread do not mark anything read automatically; (3) a targeted Reply to a specific message (one that establishes the linkage, at least in our client) marks that message read, in Main or inside a thread.  (Reserved on main 2026-10-14; the work is on a branch and lands under this number.)
