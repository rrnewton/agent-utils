---
title: 'documents-reader-layout: give the reader heading rules, a reading measure, zebra tables, and no dead grip or channel menu items'
status: open
priority: 1
issue_type: task
assignee: mu-docread
created_at: 2026-11-10T17:56:44+00:00
updated_at: 2026-11-10T17:56:44+00:00
---

# Description

[watermelon-full-preview] Owner reply 338 (2026-11-10), comparing the Documents reader with the code host's rendering of the same document (`ai_docs/2026-11-10-vibe-talk-redraw-model.md`): sections there carry a horizontal rule under each heading and ours do not; our lists lack the whitespace around the list with dense items inside it; document text has no column width limit, so on desktop the lines just grow and grow; the vertical reading-width bar from the message view is still drawn over the reader, does nothing there, and the text spills over and overlaps it; tables lack the slight background highlighting of every other row; and the hamburger menu still offers "Expand all" in the document view, where it makes no sense.

Verified state: `vibe-talk/web/styles/voice/21-documents.css` gives `#doc-body` padding and table borders only — no measure, no heading dress, no list spacing, no zebra. The bar is `#width-grip` (the reading-width handle), an overlay sibling of the scroller with no per-view gate, while `#doc-body` ignores `--reading-width`, so in the Documents view the handle stands over text it does not govern. The menu items are the channel list's (`#expand-all` and friends); their visibility is computed from channel folds alone, never from which view is on screen.

# Design

- Reader dress in `21-documents.css`, matching the host's document reading: a bottom rule under `h1`/`h2` with heading spacing; whitespace before and after a list with dense items (small item margins, prose indented under the bullet by the list's own padding); zebra rows (every other body row on a slight ground, readable in both colour schemes).
- A reading measure for document text on desktop: `#doc-body` bounded to about 90ch and centred, so lines stop growing with the window; a phone keeps the full width. The bound is the reader's own, not the message column's `--reading-width`, because the grip's contract is the message column.
- The grip never stands over the reader: hide `#width-grip` while the Documents view is on screen, by the same `:has(#pane-docs:not([hidden]))` gate that already hides `#search-float` there. Text overlapping the grip becomes impossible rather than merely unlikely.
- Channel-only menu items ("Expand all" first among them) are not offered while the Documents view is on screen. The hide must be the `hidden` state the action menu counts, not CSS dress alone, so the menu's empty line stays truthful; the view switch is where it is applied and undone.
- Files: `vibe-talk/web/styles/voice/21-documents.css`, the view-switch/menu code that owns the items' `hidden` state, and the documents browser check. Touch `20-desktop-regime.css` only if the grip gate cannot live in `21-documents.css`. No read-path files.

# Acceptance Criteria

- Browser checks with real input at desktop and 393x852 (touch), against the real server documents API, opening a document with headings, a wrapped list, and a table:
  - on desktop the reader column is measurably bounded (well under the window width) and centred; on the phone it takes the screen;
  - no document text sits under the grip: the grip has no box in this view, and `elementFromPoint` over the text column never returns it;
  - table body rows alternate grounds (computed backgrounds differ between adjacent rows);
  - headings carry their rule (computed bottom border on `h1`/`h2`);
  - the hamburger menu in the Documents view offers no "Expand all" (and offers it again back in the channel view when folds exist).
- The message view is unchanged: the grip still governs the message column, and the channel menu still offers its items there.

# Notes

[watermelon-full-preview] mu-docread, owner reply 338 batch, piece B of three. Siblings: #1235 documents-list-continuation, #1237 documents-collapsible-sections. Prior work: #1038 documents-outline-popup (same stylesheet and pane), the width grip's own history, #820 voice-page-action-menu. If the grip gate needs an overlay stacking change, follow the z-index scale owned by #1228 menu-above-chips and tell that lane. Page-only change: path-selected validation covers it.
