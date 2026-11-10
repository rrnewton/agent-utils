---
title: 'documents-list-continuation: a wrapped list item spills its tail as a separate paragraph in the reader'
status: closed
priority: 1
issue_type: task
assignee: mu-docread
created_at: 2026-11-10T17:56:44+00:00
updated_at: 2026-11-10T20:43:44+00:00
closed_at: 2026-11-10T20:43:44+00:00
---

# Description

[watermelon-full-preview] Owner reply 338 (2026-11-10): in the Documents reader, a dense bullet list whose items wrap over source lines renders busted. Each longer item's wrapped tail becomes a separate paragraph of its own — whitespace before it, all the way left-indented, not indented to the right of the bullet. The code host renders the same document as one dense list with proper wrapping. The reference document is `ai_docs/2026-11-10-vibe-talk-redraw-model.md`; the list is source lines 34-53, plain CommonMark: `- item` lines with 2-space-indented continuation lines.

Verified cause (correcting the first guess that server chat rendering leaked into the reader): the reader never runs the server renderer. `GET /api/v1/documents/{key}` (`vibe-talk/src/http/api.rs` `get_document`) sends the fetched Markdown raw, and the page renders it with the client renderer `renderDocumentMarkdown` in `vibe-talk/web/src/view/documents/render.ts` (called at `reader.ts`). That renderer's list branch trims every line and accepts only lines that themselves open an item; an indented continuation line fails the item pattern, ends the list, and falls through to the paragraph branch — exactly the photographed spill. The server's only document pass is read-aloud (`spoken_blocks_for` with `Markup::default()`, the CommonMark dialect), so `vibe-talk/src/render.rs` and its chat passes are not in this bug and must not be touched for it.

# Design

- In `renderDocumentMarkdown`'s list handling, a non-blank indented line that does not open a new item (and does not open another block) is a continuation of the current item: it joins that item's text with a space, so the whole item is one `<li>` and inline formatting still applies across the join. A blank line, a new item of either list kind, a heading, a table start, a fence, a quote, or a rule ends the list as today. An unindented prose line straight after an item ends the list and becomes its own paragraph (the reading the page has always given documents; CommonMark's lazy continuation is deliberately not adopted, so a caption under a list stays a caption).
- Scope is continuation joining only. True nested sub-lists keep today's flat reading (an indented item marker is still an item of the flat list); nesting is separate work if wanted.
- Files: `vibe-talk/web/src/view/documents/render.ts`, `vibe-talk/tests/js/view/documents.test.mjs`, and the documents browser check that exercises the reader body. No `voice.js`, no server files.

# Acceptance Criteria

- Unit tests in `vibe-talk/tests/js/view/documents.test.mjs` pin the reference list's exact shape: the lines 34-53 list renders as one `<ul>` with one `<li>` per source item, each item's wrapped tail inside its own `<li>`, no `<p>` between items; plus the boundary cases (blank line ends the list, unindented prose after an item is a paragraph, an ordered list item's continuation joins the same way, a list at the end of input).
- A browser check against the real server documents API opens a document holding that list and observes one list element with the full item text in each item, at desktop and phone widths.
- The renderer's safety contract is unchanged: source HTML stays escaped text, and the existing malicious-input tests stay green.

# Notes

[watermelon-full-preview] mu-docread, owner reply 338 batch, piece A of three. Siblings: #1236 documents-reader-layout, #1237 documents-collapsible-sections. Prior reader work: #1038 documents-outline-popup, #1039 documents-read-parity, #628 documents-index-server. The server-side list rules of #237 nested-lists are chat-message rules and do not apply here. Page-only change: path-selected validation covers it. The hand-rolled renderer's other GFM gaps (nested and loose lists, lazy continuation, code in items, ordered start-at-N, task lists, autolinks) are audited against real documents under this issue and weighed for the longer term in #1238 documents-markdown-library (hand-rolled vs a client library vs server comrak + ammonia in a plain-GFM document mode); this issue fixes only the continuation spill.

## Resolution

Landed as c70a94a2 on github/main (be1fe1bc replayed from its fork point). The list branch of `renderDocumentMarkdown` now joins an indented continuation line to its item with a space; blank lines, other blocks, other-kind items, and unindented prose still end the list. Evidence: the reference 7-item list rendered 7 one-item lists plus 7 paragraphs before and 1 list of 7 items after (probe and unit tests); `documents.test.mjs` 44/44 including 3 tests observed failing first; `documents_browser.py` gained the wrapped-list reader assertion and passed in the gate. Gate accounting: the owner-tier path-selected gate on be1fe1bc finished 153 passed / 1 failed, the sole failure `vibe-talk.check.fold-latency-browser`, classified as a shared-load flake on untouched code (three owner gates concurrent; the step missed its 28 ms budget at load x1.55 here and passed at x1.00 inside #1236's green run; the same step failed on unrelated heads the same afternoon and has its own #1231 fold-latency-deflake). Landed on the coordinator's direction under owner reply 346 (real-time-sensitive checks must not gate; being disabled separately), with the queued solo rerun cancelled. Unverified: nothing else; the renderer's remaining GFM gaps are #1238 documents-markdown-library's to weigh.
