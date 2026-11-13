---
title: 'read-next-finish: Next on the last sentence finishes and marks read'
status: closed
priority: 1
issue_type: task
external_ref: gh-310
created_at: 2026-11-07T23:52:46.086257463+00:00
updated_at: 2026-11-07T23:52:46.086257463+00:00
---

# Description

GitHub issue #310 read-next-finish tracks this for outside readers.

## Description

Owner (reply 205): with Autoplay off, spamming Next (right arrow) reaches the last sentence and further presses do nothing. Pressing Next there should finish the message and mark it read.

## Design

stepReading in web/src/view/voice/read-aloud.ts finishes through the natural-end path (endReadGoingOn + goOnAfterRead with nextAfterRead consulted), so Autoplay on still advances and off marks Done. On-screen Next, media nexttrack and the arrow share that function.

## Acceptance Criteria

Autoplay off Next-at-last stops + Done; Autoplay on unchanged; media key equals arrow/button; browser check with real keys at phone + desktop.

## Notes

[watermelon-full-preview] mu-docview.

## Resolution

[watermelon-full-preview] mu-readnext. `stepReading` already finished through the natural-end path at the last block (e9348463), but `renderReadSteer` disabled the on-screen Next there, so the button press never reached it and the reader stranded on the last block (owner 046). `read-next` now stays enabled on the last block: Autoplay off stops and marks Done, Autoplay on goes on to the next message, and a document/Discuss read (kind document) ends with nothing archived — the same endings as the natural end, shared by the button, the media nexttrack key and the arrow. Module tests cover Autoplay off/on and the document kind; the browser checks press the real button at phone and desktop width.
