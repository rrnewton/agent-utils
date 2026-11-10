---
title: 'documents-read-parity: give the document reader the message read-aloud controls'
status: closed
priority: 1
issue_type: task
assignee: mu-docreader
created_at: 2026-11-08T01:19:06+00:00
updated_at: 2026-11-10T06:29:28.691783261+00:00
closed_at: 2026-11-10T06:29:28.691783061+00:00
---

# Description

Owner (thread 214, reply 213): document Read mode is busted compared to the message read interface. It has zero pop-up scrubber and no button to control device/cloud reading. The experience should be approximately the same as reading messages.

Today `view/documents/reader.ts` speaks the document by sending each `plainChunks()` chunk to `deps.speak` in order: no scrubber, no device/cloud (read audio source) toggle, no Next/Prev by block, no media keys, no pause. The message read path in `view/voice/read-aloud.ts` plus `view/voice/readcontrols.ts` has all of these, including the pop-up scrubber/read strip, pace control, block and section stepping, media-session keys, and #1035 read-next-finish (Next on the last sentence finishes).

# Design

- Reuse the message read-aloud owner; do not fork it. Add a small document-blocks entry point or shared hooks in `view/voice/read-aloud.ts` / `readcontrols.ts` (web/src modules only; no voice.js edit without naming the range to the coordinator first) so the reader reads a document's blocks through the same transport, scrubber, device/cloud source selection, pace, Next/Prev, section stepping, media keys, and finish-on-last behaviour.
- A document read never archives or dismisses anything on natural end (the message path's archive-on-ended stays message-only), and stopping/backing out of the reader stops only the document read, as today.
- The reader's plain-text chunking aligns with the shared block machinery where practical; headings map to sections so section stepping follows the outline.
- Modules: `vibe-talk/web/src/view/documents/reader.ts` and `index.ts`, `vibe-talk/web/src/view/voice/read-aloud.ts` / `readcontrols.ts` (hooks named in the landing report), page bootstrap wiring, `vibe-talk/web/voice.html` and styles only if the shared strip needs surfacing in the reader.

# Acceptance Criteria

- Browser checks with real input at desktop and 393x852 (touch), against the real server API: reading a document shows the pop-up scrubber, the device/cloud toggle switches the audio source, Next/Prev move by block (buttons, arrows, and media keys agree), Next on the last block finishes the read, pause/resume work, and Back stops the read.
- Message reading is unchanged: existing read-aloud browser checks still pass, and a finished document read marks no message read/done.

# Notes

[watermelon-full-preview] mu-docreader, thread 214 batch B. Sibling work: #1038 documents-outline-popup, #1040 documents-voice-agent.

## Resolution

Landed in stages: dabe4890 (server speech tickets: POST /api/v1/documents/{key}/speech/prepare minting tickets from server-cached content only), 36576d0e (player extraction), f19485c5 (`readDocument` on the shared read-aloud owner, `Playing.kind` gating the message-only end behaviours), ae67aed1 (test-fixture fix), then stage 3: 5484c9bd + 85501d44 (the reader wired to the shared owner, kind-aware stop on view change, `docs` in the control bar's audio-source views). The reader now reads through the same singleton as messages — pop-up scrubber, device/cloud toggle, Next/Prev by block, media keys, pause/resume, and finish-on-last — with no fork of read-aloud.ts/readcontrols.ts. As designed, segmented device reads were NOT applied to messages: message device reads are deliberately unchanged, and document reads never archive, dismiss, or mark anything read. Browser evidence: documents_read_browser passes at 393x852 and 1280x800 (strip, device blocks, Next/Prev, media next, finish-on-last, device/cloud toggle, Back-stop), and the existing message read-aloud suites stayed green through every stage's gate. Stages 1+2 went live on devbig014 2026-11-08 (c178389c); stage 3 followed on main. Known v1 caveats, accepted at design time: no per-block light in the reader DOM, and documents over 16 KiB get plain-paragraph speech blocks.
