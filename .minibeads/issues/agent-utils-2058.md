---
title: 'vibe-talk-reader-arrows-instant: forward/back arrows paint the new block at once, voice follows'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- reader
- ui
- performance
created_at: 2026-11-21T10:43:00-05:00
updated_at: 2026-11-21T10:43:00-05:00
---

# Description

[watermelon-full-preview] Owner request 284, fast-track (2026-11-21 10:43 AM ET), quoted: "The forward back arrows are taking multiple seconds before they navigate to following chunk of text. Irrespective of whether reading is paused or active, I would expect forward backward navigation to be instantaneous with a clear update to the user interface that is snappy and responsive even if the voice lags behind. It seems like somehow we've gotten the voice crap on the critical path and its lag contributes to lag in our UI."

Measured await chain (before the fix): arrows/media keys/timeline -> stepReading/stepSection -> jumpReading -> readAloud/readDocument. The paint lived at the end of that async chain: stopReading cleared the light synchronously, then the handler awaited a speech prepare (ticket mint) or fetch whenever tickets were not ready, then playPreparedRead rebuilt nowPlaying and only its player 'playing' event moved the segment light. Paused jumps paid the same awaits. The deployed speech GETs (p50 124 ms, p90 14 s) are consistent with streamed audio held open and were not blamed; the client prepare is on the arrow path regardless.

# Design

- jumpReading paints synchronously, in the press's own frame: it records the target as the authoritative UI position (jumpPaint), moves the segment, and calls showReadingPosition (block light, strip count, follow scroll; documents report their block) before any speech work.
- The speech stop/restart fires after the paint, asynchronously and cancellably through the existing readingTicket: a later press overwrites the paint and bumps the ticket, and every player/await handler already drops ticket-stale audio, so late audio for an old position is never played.
- While the restart is in flight, readingPosition, markReadingBlock, showReadingPosition, showDocumentBlock and the strip read the paint; the rebuilt read clears it when it takes over. An ordinary stop clears it too.

# Acceptance Criteria

- With a slow fake TTS (held speech prepare), an arrow press moves the block highlight before the prepare resolves — asserted on condition/state, never wall-clock ms (owner rule 346) — playing and paused, channel reader and Documents reader, forward and back.
- Rapid presses land on the right block; stale audio for a superseded position is never played.
- Before/after numbers at 1x and 4x CPU: arrow-to-paint, long tasks, wall (fixture harness).
