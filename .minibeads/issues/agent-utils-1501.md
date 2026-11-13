---
title: 'discuss-read-aloud-parity: Discuss summary reads aloud like messages and documents'
status: closed
priority: 1
issue_type: feature
assignee: mu-discussread
labels:
- vibe-talk
created_at: 2026-11-13T23:06:00.000000000+00:00
updated_at: 2026-11-13T23:06:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner request 043 (11-13 6:06 PM ET), verbatim: "Okay, I'm on mobile and I just popped up in the discuss view and open the summary. But then I tried to click the read button and click on the text to read it to me. That didn't work. It absolutely should. Reading messages, reading documents and reading the discuss view should all have a similar experience and similar features. Please get this shipped ASAP so I can use it".

The Discuss summary has no read-aloud at all: no Read control starts it, and tapping its text does nothing. Messages and Documents share one read-aloud owner with its strip, device/cloud toggle, pace, Next/Prev by block, media keys, and the lit block kept in view.

# Design

- Reuse `#1039 documents-read-parity`: the Discuss panel hands ONE summary to the shared owner with `readDocument({key, title, texts, onReadingBlock})`, exactly as the Documents reader does. No second reader, no new transport.
- Stable key: the saved Discuss record identity `#1497` uses — scope (channel + thread) plus the saved summary's `created_ms` generation (`recordKeyFor`). Fresh Summarize answers use the scope with generation 0 until the saved record lands and redraws with its generation.
- Texts: ALL of the summary's drawn blocks, in order — the Documents choice (`plainChunks` reads the whole document, folds or outline never cut it). Folded Discuss sections therefore read whole, expanded or not; the drawn blocks and their texts are collected together so tap index and highlight index are the same index.
- Tap-to-read: a tap on a drawn block (not a citation, not a fold toggle, not the collapse bar) starts the read from that block through the same `readDocument(..., {part: 0, block})` from-block path the owner uses for a document jump.
- Highlight: the owner's `onReadingBlock` lights the drawn block with `data-reading-block="current"` and keeps it in view, as in the Documents reader; a null block takes the light off.
- Kind-aware stops: `stopSpeaking(key)` scoped to the Discuss key only, as the page wires it for documents. Closing Discuss, Escape, or a new Summarize stops only the Discuss read.
- Controls: a `discussOpen`-style live getter on the voice-call deps, so `renderControls` shows `#read-aloud`/`#read-speed` while Discuss stands open, as `docsReaderOpen` does for the reader. The strip, toggle, pace, Next/Prev and media keys are then the owner's own.

# Acceptance Criteria

- Read on the Discuss summary starts the shared owner's read; tapping a block starts it from that block; the strip counts blocks and Next/Prev and the media keys step it.
- The block being read is lit in the answer and kept in view; stopping, finishing, closing Discuss or a new Summarize takes the light off and stops only that read.
- Module tests cover the key, the texts, the tap index, and the stop scoping. The discuss browser check covers Read and tap-to-read on a phone viewport with condition waits only (owner rule 346), in the documents-read harness's device-voice style.

# Notes

Owner UI fast-track (owner rule 286), request 043. Lane mu-discussread from coordb. Documents parity from `#1039`; record identity from `#1497`. Gate: owner tier (GATE_PRIORITY=owner), then land-main and coordb deploys at once.

## Resolution

The Discuss summary now reads through the shared read-aloud owner exactly as a document does: Read hands it one `readDocument({key, title, texts, kinds, onReadingBlock})` call under the saved record's key (scope plus `created_ms`), with texts and kinds taken from the one drawn-block walk that also serves tap index and highlight index — all blocks, folded sections included, as Documents reads whole. Tapping a block starts from that block; the owner's strip (floated above the panel), device/cloud toggle, pace, Next/Prev and media keys steer it; the block being read is lit and kept in view; closing Discuss or a new Summarize stops only that key. Module tests cover the key, texts, kinds, tap index, highlight and stop scoping, and the discuss browser walk covers Read, Next, tap-to-read and stop-on-close on the phone viewports with the documents-read device-voice harness.
