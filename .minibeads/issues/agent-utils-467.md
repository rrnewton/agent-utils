---
title: 'inline-send-in-place: a reply sent from an inline composer keeps its outgoing card where the composer was'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- send
depends_on:
  agent-utils-448: related
created_at: 2026-10-23T17:55:10.809513941+00:00
updated_at: 2026-10-23T17:55:10.809513941+00:00
---

# Description

[opus 5.5] The owner: "The compose box jumps to where it should be, in situ in the thread... But then the little 'Sending..' box, the one that would show Retry if it fails, jumps back down to the BOTTOM, not where the compose box was. This should provide better continuity."
Wanted: when a reply is sent from an inline (in-thread) composer, its outgoing card (Sending… / Posting part 2 of 3… / Not sent with Retry and Edit) appears where the composer was, inside that thread, and stays there until the real message replaces it in the same spot, with no scroll jump. The same in every view with an inline composer (the Threads view cards, a thread, two columns). A main-channel send keeps its card at the foot. Page tests and a Chromium walk.
