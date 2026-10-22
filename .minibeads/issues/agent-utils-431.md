---
title: 'voice-transcript-expand: a long voice turn opens and closes with a tap, and stays as the reader left it while it grows'
status: closed
priority: 1
issue_type: bug
created_at: 2026-10-22T15:45:38.456437457+00:00
updated_at: 2026-10-22T19:59:25.781899704+00:00
closed_at: 2026-10-22T19:59:25.781899594+00:00
---

# Description

[muse watermelon] The owner, using the voice agent from a car: a long spoken reply's card in the voice transcript was collapsed, clipping the end of the message, and tapping it did not expand it.

Two causes in web/voice.js:

1. A channel row is its own fold control (`tapRow`): a tap anywhere on the message folds or unfolds it, and the small fold button is kept only for the keyboard. A transcript turn never got that handler, so a tap on the card did nothing; only a small More pill on its metadata line worked.
2. A streamed turn (`vibe-talk-v1`) is redrawn from scratch on every frame (`drawSpoken` -> `turnNode` -> `foldable`), and a fresh row starts folded. A voice turn has no message id, so `expandedIds` cannot carry it. A reply that grew past the fold threshold while being spoken snapped closed under the reader, and one the reader had opened closed again on the next frame.

Wanted: the channel's idiom in the transcript, through the same code path (tap the message to open it, tap again to close it; the fold button kept for the keyboard); and a growing turn keeps the state it is in. A turn shown whole while it was short is never folded by growing, and a turn the reader opened or closed stays that way. Following the newest line, and keeping the reader's place when a card opens, work as they do in the channel.
