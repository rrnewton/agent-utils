---
title: 'stream-trailing-byte: do not silently drop a partial final sample in read-aloud streaming'
status: in_progress
priority: 2
issue_type: bug
assignee: opus-5.5/stream-trailing-byte
labels:
- vibe-talk
- audio
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-26T10:29:04.019315230+00:00
updated_at: 2026-09-26T10:37:37.315836586+00:00
---

# Description

[opus 5.5] Found in the final doneness audit for #1 mobile-chat-reader.

The streamed read-aloud player in vibe-talk/web/voice.js decodes whole PCM frames and keeps any remainder in `carry` for the next chunk. When the stream ends, a non-empty `carry` is discarded without a word. An odd final byte, or on stereo any partial frame, is dropped, and the read still counts as finished. The page archives a finished read as heard. So a body cut short by one byte, or a WAV whose data length disagrees with its sample size, passes as a clean end.

# Acceptance Criteria

At end of stream, a non-empty remainder is handled explicitly. Either the read is treated as not having arrived whole (the same path as a response cut off part-way, so the message is not archived), or the remainder is logged and the choice is documented where `carry` is kept. Regression tests cover an even-length body (plays, ends, archives), an odd trailing byte, and a partial stereo frame, and they fail without the fix.
