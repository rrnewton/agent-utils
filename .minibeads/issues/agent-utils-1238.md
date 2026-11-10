---
title: 'documents-markdown-library: weigh a vetted GFM renderer for the reader against the hand-rolled one'
status: open
priority: 2
issue_type: task
assignee: mu-docread
created_at: 2026-11-10T18:00:58+00:00
updated_at: 2026-11-10T18:00:58+00:00
---

# Description

[watermelon-full-preview] Follow-up from #1235 documents-list-continuation (owner reply 338 batch, 2026-11-10). The Documents reader renders fetched Markdown with a hand-rolled client renderer, `renderDocumentMarkdown` in `vibe-talk/web/src/view/documents/render.ts`: escape the whole source, then apply line-based block rules and a small inline pass. That design has one real virtue — untrusted document HTML can never reach the page, because the source's angle brackets are escaped before any formatting — but it reimplements a subset of CommonMark/GFM by pattern matching, and the subset keeps being found by its edges. #1235 fixes the photographed edge (a wrapped list item's continuation spilling as a paragraph). This issue weighs three grounds for the reader's rendering: continuing hand-rolled, a vetted CommonMark/GFM library compiled into the page with sanitized output, and rendering on the server with the existing comrak + ammonia pipeline in a plain-GFM document mode. A recommendation is the deliverable; no build under this issue.

GFM constructs the hand-rolled renderer is expected to get wrong, from reading it (to be confirmed empirically against `ai_docs/2026-11-10-vibe-talk-redraw-model.md` and two other `ai_docs` documents during #1235, results recorded here):

- Nested lists: every line is trimmed, so an indented sub-item is drawn flat at the top level.
- Loose lists: a blank line between items ends the list, so a loose list becomes many one-item lists (and multi-paragraph items are impossible).
- Lazy continuation: an unindented line after an item ends the list, where CommonMark continues the item's paragraph. (Partly deliberate — a caption stays a caption — but it diverges from the code host.)
- Code blocks inside list items: a fence after an item ends the list instead of belonging to the item.
- Ordered lists starting at N: the start number is dropped; every list restarts at 1.
- Task lists: `- [ ]` / `- [x]` render as literal bracket text, no checkbox.
- Autolinks: a bare address in prose is never linkified; only bracketed links are.
- Also from the same reading: no setext headings, no reference-style links, no strikethrough, no underscore emphasis, table column alignment ignored, blockquote lines flattened into one paragraph, no backslash escapes.

# Design

DESIGN FIRST, decided before any code. Three options, weighed with evidence from the real documents agents actually link:

1. Hand-rolled page renderer (today): keep `renderDocumentMarkdown` and fix the audited constructs one by one. Each construct above is a small parser, and the line-based shape resists some of them (nesting, loose lists) structurally; price that honestly.
2. A client library: a vetted CommonMark/GFM renderer compiled into the voice page, its output sanitized in the page (the maintained names in this space include markdown-it, marked, and micromark; the evaluation names its own shortlist and rejects with reasons).
3. Server comrak + ammonia: render documents with the existing pipeline in `vibe-talk/src/render.rs` (comrak 0.55 writes the HTML, ammonia decides what of it may exist, from an empty allowlist) in a plain-GFM document mode — no chat passes (no list-ending rule, no chat block repairs), soft line breaks — and send the sanitized HTML with the document. One renderer for messages and documents, already sanitized and GFM-correct; the document mode's tag and attribute needs (heading ids the outline jumps to, table structure) must be stated against the sanitizer's current allowlist.

Weigh all three on the same axes:

- Correctness: render the audit corpus through each option and count divergences from the code host's reading per construct above, so the choice is priced in wrong output, not taste.
- Safety model: document content is untrusted exactly as channel text is. Today's contract — the source's own markup never reaches the page, link/image addresses held to an allowlist — must survive on every option, including the in-app document-link resolution of `resolveDocumentHref` (`#1081 documents-relative-links`), reader behaviour no off-the-shelf renderer provides. For the server option, note what changes shape: the page would insert server-sent HTML for documents as it already does for messages, instead of building every tag itself.
- Bundle size: what each client option adds compiled into the voice page; the server option adds nothing to the page and lets the hand-rolled renderer be deleted.
- Offline rendering of saved documents: a document the page already holds (fetched earlier, server now unreachable) can still be drawn by options 1 and 2 from its stored Markdown; option 3 must say what the page holds — stored Markdown it cannot draw offline, stored sanitized HTML alongside it, or a client fallback — and what that costs.
- Paste/gist and other non-repository sources: documents arrive from more than one kind of source (`#1176 doc-kind-buckets`); each option must render every kind the same way, with no source privileged except where link resolution already is.
- Latency: time from opening a document to drawn text — client render of a large document on a phone, against a server round trip plus server render, for a first open and a reopen.
- Fit: build integration with the existing TypeScript page build (client options), licence and maintenance state (library option), and spec compliance evidence for each.

# Acceptance Criteria

- An audit table in Notes: each construct above, hand-rolled behaviour today (observed on the corpus, not just read off the code), and how often the corpus actually uses it.
- A recommendation in Notes with the evidence, one of the three: stay hand-rolled (with the construct-fix list, each filed or folded into a named issue), adopt a named client library (with its sanitization design and bundle cost), or render server-side in the plain-GFM document mode (with its offline and stored-HTML answer and sanitizer-allowlist deltas stated).
- No build under this issue; if the recommendation changes the renderer, it lands as its own issue with the malicious-input tests of `vibe-talk/tests/js/view/documents.test.mjs` carried over unchanged.

# Notes

Audit observed 2026-11-10 under #1235 (hand-rolled renderer, before that fix; corpus: the redraw-model document's list, `ai_docs/dagrun-profile-feedback-audit.md`, `ai_docs/2026-07-29-parallel-experiment-runner-design.md`, plus one probe per construct):

- Wrapped continuation lines: spilled as separate paragraphs (the 7-item reference list drew as 7 one-item lists + 7 paragraphs). Fixed by #1235: 1 list, 7 items, 0 paragraphs. On the two full corpus documents the fix kept every item (92 and 67) while list elements fell 60 to 21 and 38 to 14 and paragraphs fell 126 to 77 and 75 to 41 — the spills rejoined their items.
- Nested lists: drawn flat (indented sub-items become top-level items).
- Loose lists: each item its own one-item list (blank line ends the list).
- Lazy continuation: an unindented line ends the list (diverges from the host; kept deliberately in #1235 so a caption stays a caption).
- Code fence inside an item: splits the list around the code block.
- Ordered start-at-N: start number dropped, always renumbers from 1.
- Task lists: bracket text literal, no checkbox.
- Autolinks: bare addresses stay text.
- Strikethrough, underscore emphasis, setext headings, reference-style links: literal or plain-paragraph output.
- Table column alignment: ignored (structure renders). Blockquotes: lines flattened into one paragraph.

[watermelon-full-preview] mu-docread. Parent: #1235 documents-list-continuation. Related: #1236 documents-reader-layout (the dress any renderer output wears), #1038 documents-outline-popup (heading ids any renderer must keep agreeing with). P2: the fast-tracked fixes land first; this decides the reader's longer-term rendering ground.
