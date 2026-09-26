---
title: 'post-confirmation-card-hold: hold Send when a confirmation card appears where none was shown'
status: in_progress
priority: 2
issue_type: bug
assignee: opus-5.5/post-confirmation-card-hold
labels:
- vibe-talk
- ui
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-26T10:29:04.019315230+00:00
updated_at: 2026-09-26T10:37:37.315836586+00:00
---

# Description

[opus 5.5] Found in the final doneness audit for #1 mobile-chat-reader, in the #34 voice-chat-write-confirm post gate.

`showPostProposal` in vibe-talk/web/voice.js holds Send for `POST_CHANGE_HOLD_MS` only when a proposal REPLACES one already on screen. When a card appears where none was shown, `previous` is null, no hold is set, and Send is enabled on the first frame the card is visible. A tap already on its way to whatever was under that spot, such as a message row or the transcript, can land on Send and post text the owner has not read.

# Acceptance Criteria

A card that appears where none was shown keeps Send disabled for the same hold as a replaced card, then enables it without further input. A replaced card keeps its hold and its "text changed" notice. Don't send stays usable throughout. A real-browser regression taps Send immediately after a first card appears and shows nothing is posted, and it fails without the fix.
