---
title: 'documents-scrubber-multilevel: finish the multi-level document scrubber (section tree, numbers, two-row navigator)'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- documents
depends_on:
  agent-utils-629: blocks
created_at: 2026-11-14T00:00:00+00:00
updated_at: 2026-11-14T00:00:00+00:00
---

# Description

Owner request 050 (11-13 6:29 PM ET): after the reader P0s, revisit and FINISH the multi-level document scrubber from the approved design in `ai_docs/plans/2026-10-26-documents-view/README.md` (scrubber idioms ~595-700, "Section numbers" ~636, "Decision: a focus-plus-context row over the block row (owner 2026-10-28)" ~718). The navigator part of #629 documents-view-client was never built. Today a large document shows a single-track scrubber with uselessly tiny chunks.

Owner additions:
(a) render section numbers ON THE DOCUMENT itself (1, 1.1, 1.2 ... before each heading, as if the document always had them), unless the document already has detectable numbers or letters (the design's "author's numbers win" rule);
(b) the same numbers appear on the second scrubber row;
(c) a minimum chunk size.

Build on what exists: the Documents reader (view/documents), the #1505 block model (`documentReadBlocks`, one block list for speak and highlight; tap-to-read), and the existing single-track scrubber. The navigator, section numbers and read-aloud must all use the SAME block and section model, so tapping a scrubber unit starts reading there and the highlight follows.

Suggested landings, each deployable on its own:
(1) the section tree + numbers on the document (gutter labels before headings; author's numbers win) + a minimum chunk size on today's scrubber;
(2) the two-row focus+context navigator on a phone (ranges, boundary marks, the numbers on row 2);
(3) the third desktop row and polish.

# Acceptance Criteria

Module tests for section numbering (title rule, skipped levels, Introduction, author-numbered detection incl. 3., 3.2, III., A.), aggregation to K_target with the minimum chunk size, range compression, and boundary marks; browser checks at phone and desktop width with a large synthetic document (hundreds of blocks, nested sections, one huge leaf section): rows count, tap a unit -> reading starts there, numbers on the document. No wall-clock waits (owner rule 346).

# Notes
