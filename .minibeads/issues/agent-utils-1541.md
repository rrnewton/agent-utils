---
title: 'documents-nav-outline-tree: move the two-row navigator into the scrubber, built from the outline tree'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- documents
depends_on:
  agent-utils-1508: blocks
created_at: 2026-11-14T00:00:00+00:00
updated_at: 2026-11-14T00:00:00+00:00
---

# Description

[watermelon-full-preview] Owner UI request 066 items 1-5 (2026-11-14), via the liaison. Today the reader's two-row navigator (`#1508 documents-scrubber-multilevel`) stands always-on at the top of the reader and scrolls away; its rows show raw global chunk indices (`0..6`, `0 3 5 9 11 12 13`) and its "sections" appear to jump to random paragraphs; separately the orange read scrubber draws one tiny block per speech segment (159 in the owner's document) spilling off both screen edges. The owner asks: (1) the two rows live in the orange scrubber that pops up — row 1 section navigation, row 2 the chunks inside the selected section; (2) no raw global indices; (3) build the navigation from the Outline's tree — section headers form a clean tree, leaf sections are flat lists of text chunks, and table text is ordered after everything before the table and before everything after it; (4) revisit the "you are here" rule — unfold the tree only along the path to the current node, keep the siblings that unfolding showed, show "..." for non-adjacent ones, highlight the current section (e.g. 1.2.3); (5) the orange margin count ("X of Y") is fine as it is. Owner 071 adds the look: the innermost clickable chunks are rectangular blocks like the bottom timeline's, and nested sections may show as nested thin rectangular outlines with slightly different background colours.

Root cause of the "random paragraph" jumps, from the code: leaf cells are aggregated spans (heading plus what follows, whole list/quote/table runs, anything under the minimum chunk merged onward, then balanced groups), labelled in the reader with the span's global start index. The label reads as a chunk number but the tap lands at the span's first block, often a heading or list start well away from the paragraph the number suggested. The block order itself (`blocks.ts`, `#1505 documents-read-controls-highlight-tap`) is strict document order and a table is already one block at its place; the disorder is introduced by aggregation and labelling, not by the block cut.

# Design

Reconciled with the governing design (`ai_docs/plans/2026-10-26-documents-view/README.md`, `#544 documents-view-design` / `#629 documents-view-client`, navigator decision owner 2026-10-28), `#1508` as built, `#1505`'s one block model, `#627 scrubber-thumb-arrows` (the steer row's fat/thin arrows and "k of n", untouched), and `#1237 documents-collapsible-sections` (body folding, still open; this issue folds nothing in the body). No server change: the reader already holds the full section tree client-side (`buildSections` over the heading blocks; the server outline stays a flat heading list for the pop-up and notice).

**Data model (pure, in `sections.ts`).** Reuse `buildSections` unchanged — title unnumbered, depth as ancestor rank, section 0 Introduction, author's numbers win per level — as the Outline tree: `sections[]` with `number`, `title`, `depth`, `[start, end)`, `parent`, `children`. New pure model over it: the current section is `sectionOfBlock[currentBlock]` (deepest containing section); a section's own chunks are the blocks of `[section.start, firstChild.start ?? section.end)` — for a leaf, all its blocks; for a parent, its text before its first subsection. One chunk is exactly one block. Reading order is the block model's document order end to end, so a table block sits between the blocks before and after it by construction; speech may subdivide a block into pieces at speak time, but navigation, the light and the reported position all stay in block indices (`#1505`).

**The two scrubber rows.** `#doc-nav` moves from the reader's head into `#read-steer` (the orange strip), between the arrows row and `#read-timeline`; while a document read stands the flat per-segment timeline is hidden and these rows take its place, so the 159-cell spill cannot recur. Message reads keep the timeline exactly as today; "X of Y" stays. Row 1 is the unfolded section tree (below); row 2 is the current section's chunks, labelled with their local paragraph numbers (`¶ 1..n` within the section), never global indices. Desktop also gets exactly these two rows. Look (owner 071): innermost chunks are rectangular blocks in the timeline's language (thin orange border, heard tinted, current filled); expanded sections are thin rectangular outlines nested inside their parent's outline, background tint deepening slightly with depth, each carrying its number as a small clickable label:

```text
#read-steer (390 px):  [Auto]  ⇦ ⇨  42 of 159  ← →        [React]
row 1  ┌─1────────────────────┐ ┌2┐ … ┌5┐
       │ 1.1 ┌─1.2───────────┐│
       │     │ 1.2.1 ▓1.2.2 …││   ▓ = current section 1.2.2, filled
       └─────┴───────────────┘└─┘   └─┘
row 2  [¶1][…][¶6][¶7][▓¶8][¶9][¶10][…][¶24]   chunks of 1.2.2
```

Tapping a section label starts reading at that section's first block; tapping a chunk starts at that block; tapping "..." starts at the first hidden item of its run, which re-centres the window there. All taps go through the reader's existing `startReading`, so the light follows in the same index space.

**You-are-here rule.** Visible sections are exactly what unfolding along the path shows: a section is visible when its parent is the root or an ancestor-or-self of the current section; ancestors of the current section are expanded (drawn as outlines holding their visible children), nothing else is. Each sibling list is then windowed to fit: the child on the path (or the current section), its nearest siblings, and the list's first and last entries are shown; every maximal run of the rest becomes one "..." cell. Row 2 windows its chunk list by the same rule around the current chunk. Cell caps come from the strip width at the design's 24 px target size, so both rows fit a 390 px screen without horizontal spill. Invariants, each unit-tested: (I1) document order everywhere, never reordered; (I2) the current section and chunk are always shown, exactly one current per row; (I3) every ancestor expanded, no non-ancestor expanded; (I4) each hidden run is exactly one "...", never two in a row; (I5) cells per row never exceed the width cap; (I6) a cell's label names exactly its tap target — a section's number, a chunk's single block; (I7) every section and chunk is reachable by repeated taps; (I8) a table chunk lies between its neighbouring chunks.

**What changes vs `#1508` as built.** Kept: the section tree and numbers (on the document, in the outline), the one block model, tap-to-read, 24 px sizing. Changed by owner's 066: placement (reader head -> scrubber pop-up); labels (global span starts -> section numbers and local paragraph numbers); chunk definition (aggregated spans -> one block); overflow strategy (merge runs, minimum-chunk merging, synthesized `3.3 (2/4)` parts and `6 … 15` ranges -> path unfolding plus "..." windowing); rows (focus+context fisheye, 2 on phone / 3 on desktop, with boundary marks, edge placeholders and a breadcrumb -> exactly two rows of nested outlines over rectangular chunk blocks). The navigator half of `#1508`'s model (`buildNavigator` and its aggregation) is replaced, not layered over. Non-goals: the outline pop-up (`#1542`), the control bar (`#1544`), body folding (`#1237`), the fat arrows' own section definition in read steering, and any change to the legacy page script or the app entry while `#624` moves them.

# Acceptance Criteria

Unit tests on the pure model (`tests/js/view/documents-sections.test.mjs`): tree order and chunk ranges including a table between paragraphs (table chunk between its neighbours, tap target is the table block); you-are-here unfolding (ancestors expanded only, siblings kept, "..." runs); windowing caps at phone width; exact-landing (section cell start equals the section start, chunk cell holds one block); numbering behaviour from `#1508` unchanged. Browser test (`tests/documents_scrubber_browser.py`) against the real server at 393x852 and desktop: the navigator is absent from the reader head and present inside the scrubber while reading; both rows fit on screen (no horizontal overflow); row 1 shows section numbers with the current highlighted, row 2 shows local chunk numbers; tapping a section and a chunk lands the reading light and the spoken text on the right paragraph, including in a document with a table (the table chunk between its neighbours); screenshots at both widths inspected. No wall-clock waits (owner rule 346). Capped local validate and the owner gate green on the exact landed HEAD.

# Notes

[watermelon-full-preview] Lane mu-docnav, owner UI fast track (request 066). Sibling lanes: mu-outlinelist #1542 documents-outline-list (outline pop-up rendering and stay-open only), mu-docbar #1544 (control bar and pop-up placement). Agreed split with #1542: this lane does not touch the outline drawing or its styles; #1542 does not touch the section model, navigator or scrubber. Whoever lands second rebases onto the new main and re-gates.
