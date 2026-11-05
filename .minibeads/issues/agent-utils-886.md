---
title: 'voice-page-autoplay: React returns to the right edge, and an Autoplay toggle reads the next unread message'
status: open
priority: 0
issue_type: feature
assignee: cl-refactor
labels:
- vibe-talk
created_at: 2026-11-05T13:48:07.667621857+00:00
updated_at: 2026-11-05T13:48:07.667621857+00:00
---

# Description

[opus 5.5] OWNER REQUEST (2026-11-05, 8:02 AM ET), ASAP plus deploy. The owner's words, verbatim: "Okay, when we made the react button orange we also brought it in to cluster against the center controls but I wanted to leave it on the edge right justified. Please put it back. Also add a little toggle on the left side of this control bar for autoplay. When autoplay is on at the end of a message, we will automatically keep reading the next unread message in the channel order. I.e whatever view we're on, it's the next unread message that is below us in that view. As with my other user interface request, please implement this ASAP and deploy it ASAP so I can get the new version and use it"

It follows #880 voice-page-menu-placement (Hide reader). It is an owner-approved exception between #624 voice-page-modularity batches.

# Design

1. The React button keeps its orange colour and returns to the right edge of the reader bar, right-justified, apart from the centre controls.
2. An Autoplay toggle sits at the left end of the reader bar.
   - When a read-aloud of a message ends with autoplay on, reading starts on the next unread message below it, in the current view's order.
   - Messages are marked read as they are read, the same as a manual read.
   - It stops when no unread message remains below, on Stop or Hang up, or when the user starts a different read. After a user-started read, autoplay continues from that message.
   - The setting persists per device in localStorage. Default: off.
3. The hamburger menu also offers "Autoplay: on/off", so it is reachable while #880's Hide reader hides the row. During a read the row stays drawn (#880), so Stop stays reachable.
4. No conflict with the reader's h/s keys.

# Acceptance Criteria

- Placement: React at the right edge, Autoplay at the left end of the reader bar, on phone and desktop.
- The toggle and the menu item agree, and the setting survives a reload. Default off.
- The advance logic follows view order, skips read messages, and stops at the end.
- Stop, Hang up and a user-started read behave as above.
- Gate, review, land, and send the SHA to the lead for an immediate deploy.

# Notes

Relayed by the lead on 2026-11-05 (owner reply 158). Assigned to cl-refactor, ahead of #624 batch 64.
