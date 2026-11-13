---
title: 'documents-read-controls-highlight-tap: play/pause, highlight drift and tap-to-read in the Documents reader'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-13T23:34:08.018515589+00:00
updated_at: 2026-11-13T23:34:08.018515589+00:00
---

# Description

Owner 049 (11-13 6:27 PM ET), verbatim: Okay, I just tried to use the document reader view. It has some major bugs. There is no play pause controller. Also, I noticed it was reading through bulletin order but the highlighted bit jumped way ahead beyond where it was reading just now in the X container document. And furthermore, when I try to click to read a given paragraph, my clicks were not registered at all nothing happened. These need to be p0 fixes to get out the door.

Three bugs in the Documents reader read-aloud (readAloudOwner().readDocument({key, title, texts}) since #1039):
1. NO PLAY/PAUSE while reading a document. Messages have it in the read strip. Show the same strip controls (play/pause, Prev/Next, pace, device/cloud) in the document reader, in every reader state, on phone and desktop.
2. HIGHLIGHT DRIFT: reading goes through a bulleted list in order but the lit block jumps far ahead of the spoken position. One block model for speaking and highlighting, so index N always means the same block. Reproduce with nested and long bulleted lists, headings, code and paragraphs.
3. TAP-TO-READ does nothing: tapping a paragraph starts reading from that block, through the same block model.

Coordination: mu-discussread (#1501) builds Discuss read-aloud on the same path and rebases on this fix; mu-readnext (#1035) owns Next-at-last in stepReading.

# Design

Root causes on github/main 85c994b7: (1) navigation showView hides control-pane unconditionally in docs view, hiding the Read/Pause button and pace; the strip has no pause by design (#537). (2) Three block cuts: plainChunks(markdown) blank-line chunks vs drawn documentBlocks (per-li) vs server spoken_blocks (chat markup + pieces); reader word-hunts with rotating startsWith/includes scan. (3) Reader body click only follows data-doc links; no tap-to-block path.

Fix: one pure block model documentReadBlocks(markdown) -> [{kind, text}] mirroring renderDocumentMarkdown splitting (heading/paragraph/quote/code/table one each; every list-item line one item with renderer continuation joining; hr skipped). Reader texts are the model texts, 1:1 with drawn blocks; highlight lights drawn[block] directly, word-hunt deleted. readDocument gains optional kinds aligned with texts; owner reports onReadingBlock always in texts index space (cloud server blocks aligned by kind sequence, pieces of one element collapse to one model block; unalignable reports nothing rather than a wrong light). Tap on a drawn block calls readDocument(doc, {part:0, block}) — same from-shape as #1501. Control-pane hidden in docs only while no document read stands (showView documentReading gate + renderControls sync).

# Acceptance Criteria

Module tests for the block model (list items, nested lists, headings, code; speak index == highlight index). Strip shows play/pause during a document read at phone and desktop width. Tap -> readFrom(block). Browser check (vibe-talk/tests/documents_read_browser.py extended): play/pause works, lit block matches spoken block through a long list (assert on read-state events, not time), tapping a paragraph starts there. No wall-clock waits (owner rule 346). make validate green via the owner fast-track gate.

# Notes

Lane mu-docreadfix. Coordinated with mu-discussread (#1501) and mu-readnext (#1035) via agentctl before implementation.
