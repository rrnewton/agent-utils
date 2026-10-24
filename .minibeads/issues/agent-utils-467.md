---
title: 'inline-send-in-place: a reply sent from an inline composer keeps its outgoing card where the composer was'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- send
depends_on:
  agent-utils-448: related
created_at: 2026-10-23T17:55:10.809513941+00:00
updated_at: 2026-10-24T00:50:02.703454752+00:00
closed_at: 2026-10-24T00:50:02.703454661+00:00
---

# Description

[opus 5.5] The owner: "The compose box jumps to where it should be, in situ in the thread... But then the little 'Sending..' box, the one that would show Retry if it fails, jumps back down to the BOTTOM, not where the compose box was. This should provide better continuity."
Wanted: when a reply is sent from an inline (in-thread) composer, its outgoing card (Sending… / Posting part 2 of 3… / Not sent with Retry and Edit) appears where the composer was, inside that thread, and stays there until the real message replaces it in the same spot, with no scroll jump. The same in every view with an inline composer (the Threads view cards, a thread, two columns). A main-channel send keeps its card at the foot. Page tests and a Chromium walk.

# Notes

[opus 5.5] Delivered on main in 6326e6be and c49acff9: a reply sent from the box in the stream keeps its row under the row that box was under (anchorUnder), in the column list that draws the message (a thread, Main, All, either column), anchored by CSS anchor positioning with the row keeping room, until the reply itself is drawn; a message from the foot box keeps its row at the foot; without anchor positioning, under the Pinned filter, or where the message is not drawn, the row is at the list foot. The status line says Retry is under the message it answers and no Newest chip leads to it. Tests: page tests (thread, All, foot, refused with Retry, straight after the list, Pinned) and Chromium (refused row 0-8 px under its row and as wide; a held reply in All with nothing moving as it lands). Not covered: rows unpacked under the Threads view cards in two columns (the row goes to the foot there).
