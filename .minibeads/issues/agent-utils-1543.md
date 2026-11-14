---
title: "documents-click-no-reader-start: clicking text must not start the read-aloud reader unless it is already active"
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-14T18:35:43.000000000+00:00
updated_at: 2026-11-14T18:35:43.000000000+00:00
closed_at: 2026-11-14T18:35:43.000000000+00:00
---

# Description

OWNER UI REQUEST 066 item 7 (2026-11-14): in the Documents reader, clicking or tapping text activates the read-aloud reader even when no read is under way. A plain click should select text or do nothing; it must never start a read. Only while the reader is already active may a tap on text jump the reading to that block — the tap-to-read-from-here behaviour from #1505 documents-read-controls-highlight-tap, kept intact for the active state.

The same rule was checked on the other two tap-to-read surfaces:

- Message list: already correct, no change. A row tap folds the row unless reading mode is on (`createRowGestures.actOnRow` in `web/src/view/channel/gestures.ts` gates on `readingMode()`); with reading mode on and a read playing, a tap on another block jumps it, and a card press reads only in reading mode (`packcontrols.ts`). The existing unit test in `channel.test.mjs` covers fold-when-off, read-when-on, and jump-while-playing.
- Discuss summary: had the same bug. `answerTap` in `web/src/view/catchup/panel.ts` called `readAloud(at)` on any tap on a drawn block, starting a summary read from an inactive state. Fixed here with the panel's existing `summaryReadActive` flag.
- Documents navigator and outline: not text clicks. The outline only scrolls; the navigator units and edges are explicit controls labelled "Read from …", like the Read button itself, so they still start a read deliberately. The shared read timeline buttons in the read strip are likewise explicit controls and are untouched.

# Design

Documents reader (`web/src/view/documents/reader.ts`): track this document's own read-active state. `startReading` sets it, the owner's null block report (stopped, finished or failed) clears it, `stopOwnReading` clears it, and opening a document clears it. The body-click path that would start a read from a tapped drawn block now returns early unless that state is set; marked in-document links, plain links, buttons and text selections keep their existing behaviour in both states.

Discuss summary (`web/src/view/catchup/panel.ts`): `answerTap` returns early on the block path unless `summaryReadActive` is set. That flag is already set by `readAloud`, cleared by a null block report and by `stopOwnReading`, so no new state was needed. Citation links route before this gate and still jump in either state.

Message list: no change (see Description).

# Acceptance Criteria

- Documents reader, inactive: a click/tap on document text calls the read owner zero times; the reader may select text or do nothing. Covered by the updated `#1505` unit test in `documents.test.mjs` and by `documents_read_browser.py` at 393x852 (touch) and 1280x800 (mouse).
- Documents reader, active: after Read starts a read, a click/tap on a block jumps the read to that block (`{part: 0, block: N}`); after the owner reports the read stopped, a tap starts nothing again. Same tests.
- Discuss summary, inactive: a tap on a drawn block starts no read; active: after Read, a tap jumps to that block. Covered by the updated `#1501` unit test in `catchup.test.mjs`.
- Message list: the pre-existing reading-mode gate is unchanged and its unit coverage still passes (`channel.test.mjs`).
- Browser test waits on conditions (owner rule 346), never on wall-clock budgets.

# Notes

Sibling lanes on the same page (OWNER UI 066 items for the Discuss footer, Documents nav, outline list, desktop control bar and hamburger) land independently; this change touches only the Documents reader body-tap gate, the Discuss answer-tap gate, their unit tests, the Documents and Discuss browser tests, and this issue file. `web/voice.js` and `web/src/app/main.js` are untouched.

Flake record (2026-11-14): `documents_read_browser.py` failed twice under the validate DAG's parallel load — once in `walk_list` at the tap-on-item step (fixed here by restoring the active flag on the owner's block report, after which solo runs pass) and once at the pre-existing premise `the server voice is not the picked source` (`walk()`, the `#audio-source` toggle's first `aria-checked` read, before any tap or Read behaviour this change touches). A paired A/B, interleaved and capped (cpuset 24, nice 10), ran base `0e05a600` 5/5 PASS and this head 5/5 PASS sequentially; an unrelated server-side lane's gate failed the same step the same afternoon. The premise failure is therefore a pre-existing load flake, filed as a follow-up: make that premise wait on its condition instead of asserting the first read, with no wall-clock budget (owner rule 346).
