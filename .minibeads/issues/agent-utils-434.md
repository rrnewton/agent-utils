---
title: 'read-aloud-pause: a Play/Pause toggle on the floating strip and on the Read button, one state path'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T16:11:20.346619212+00:00
updated_at: 2026-10-22T17:08:40.235701512+00:00
closed_at: 2026-10-22T17:08:40.235701422+00:00
---

# Description

[muse watermelon] Owner: we never got play/pause or resume on the Read button; can we have a pause/play button on the floating controller? Media Session pause/play already exist (#376 read-aloud-blocks) but nothing on screen drives them. Add a Play/Pause toggle to #read-steer; the Read button says Pause while a read plays and Resume while it is paused (Read off, Stop the idle way out); strip, Read button and Media Session all drive one setReadingPaused path. Pause stops audio at once and keeps the block and offset; resume continues from there, and where the voice session or speech ticket went while paused (#368 speech-ticket-gone territory) it re-prepares and restarts the paused block instead of failing. Covers Web Audio, the audio element and the device voice.

# Acceptance Criteria

Pausing from the strip, the Read button or Media Session stops the audio clock/element at once and keeps k of n and the lit block; resuming from any of them continues from the paused block and all three agree (strip label, Read label, media playbackState). A resume after the ticket expired re-prepares and reads the paused block. Page tests and a Chromium walk with screenshots.
