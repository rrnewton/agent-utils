---
title: 'vibe-talk-documents-back-and-filter-place: Back restores the exact Documents list and filters never scroll to the top'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
- ui
- scroll
created_at: 2026-11-19T10:12:00-05:00
updated_at: 2026-11-19T10:47:00-05:00
closed_at: 2026-11-19T10:47:00-05:00
---

# Description

[watermelon-full-preview] Owner report (thread 220, 2026-11-19 10:07 AM ET, verbatim): "When I filter on starred it works and I get a short list of documents. When I go into a document and then press back I am now at a filtered list that includes only that one document. That's f***** up. When I clear the starred filter I get a full list but for no reason I jump to the top of the list rather than stay near the bottom where I want to stay. Nothing should ever cause me to scroll to the top of this list automatically."

Owner screenshots (phone, dark, 2026-11-19): with Starred pressed the list reads "6 of 276 documents" and shows six cards; after opening a document and pressing Back the list reads "1 of 89 documents" and shows only that one document (the newest starred one), while the All and Starred chips both look pressed. The total changed from 276 to 89, so Back restores some other narrowing (a scope, a query, or the document's own channel) on top of Starred, not Starred alone.

Two defects in the same Documents list state code:

(a) Back from a document does not restore exactly the list state the reader left: filters, query, scope, total and scroll position. It adds a narrowing and can show chips pressed that were not the reader's filter.

(b) Changing or clearing a filter (for example clearing Starred) scrolls the Documents list to the top. Nothing should ever auto-scroll this list to the top; the reader's place must be kept.

Prior work searched before filing: #1336 doc-back-side-thread-hidden (Back from a side-column document leaves the side thread hidden; different container bug), #1265 documents-star-hide (star/hide synced state and the Starred chip), #1306 doc-star-gold (starred star visibility), and the 2026-11-10 lanes #1263 documents-list-cards (one-line cards, bottom chips, dedupe), #1264 documents-scroll-landings (first open at top, Back to bottom/exact offset), #1265 (above). Branches mu-doclist/documents-list-cards, mu-docscroll/documents-scroll and mu-docstate/documents-star-hide were checked; none restores the full filter/query/scope state on Back or preserves place across a filter change.

# Design

- Back from a document restores exactly the list state the reader left: the same kind bucket, the same Starred state, the same search query, the same scope, the same total, and the same scroll position, with no extra narrowing and only the chips that were pressed shown pressed. Opening a document must not write its own channel/scope/query into the list's saved state, and the reader's Back wiring must not re-derive the list filter from the open document.
- Nothing ever auto-scrolls the Documents list to the top. Changing or clearing a filter keeps the reader's place: when the anchor row is still in the list its rect.top stays within 2 px; when it is gone the nearest surviving row takes its place, so a reader near the bottom stays near the bottom. The fix goes through the Documents view's existing scroll surface only (no new direct scroll writer outside the scroll owner, no edit to the frozen scrollkeep/folding/foldkeys/keyboard or frame/scroller CSS files); if the right fix needed a frozen file it would stop and be named to the coordinator instead.
- Page tests at phone and desktop sizes for the exact defect, in light and dark theme where theme-related and with the keyboard open where layout-related, with condition-based waits only (owner rule 346). Dynamic testing on a real server through the real router (owner thread 185): exercise the owner's exact sequences on a test instance and attach evidence; a page test that injects a fake client call does not count. Screenshot matrix on a test instance (light and dark, keyboard open with a really shrunk viewport and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the targeted before/after states), from a fresh build rebased onto current github/main with BUILD.txt in the harness format, using the shared screenshot harness.

# Acceptance Criteria

- Sequence 1 (Starred, open a document, Back): before the fix Back shows the narrowed one-document list with a changed total; after the fix Back shows the same starred list (same count line, same total, same rows) at the same scroll place, with only the chips that were pressed shown pressed. Verified on the real server with rect.top sidecars, phone and desktop, light and dark, before from main and after from the fix tree.
- Sequence 2 (scroll near the bottom, clear Starred): before the fix the list jumps to the top; after the fix the reader's place is kept (anchor row rect.top within 2 px when the row survives, otherwise the nearest surviving row), so near the bottom it stays near the bottom. No code path auto-scrolls the Documents list to the top on a filter change.
- Existing documents and reader checks stay green; Back from main-column and side-column documents and the first-open-at-top landing are unchanged.
- Selected validation passes locally before the owner-tier gate; the gate passes or pre-existing reds are proven on a clean github/main checkout.

# Notes

[watermelon-full-preview] mu-docstate, owner thread 220, 2026-11-19. Number pre-assigned by coordb; filed by mu-docstate before implementation. Related: #1336 doc-back-side-thread-hidden, #1265 documents-star-hide, #1306 doc-star-gold, #1263 documents-list-cards, #1264 documents-scroll-landings, #1826 vibe-talk-documents-keyed-cards.

## Resolution

[watermelon-full-preview] Root cause, one code path for both symptoms: the Documents list (`vibe-talk/web/src/view/documents/list.ts`) held its rows only as far as its last read. A refresh — above all the Back-from-a-document refresh — re-read only the newest page (`limit=100`) and replaced the held rows, so every older row the reader had paged in (or that newer arrivals had pushed off the first page) silently left the list: the owner's Starred "6 of 276" came back as "1 of 89", the total in the count line being the held count. Separately, a filter change redrew the cards and kept the numeric scroll offset, which in a list whose height just changed is a different place — clearing Starred from a short list landed at the top of the full one.

Fix: `refresh()` re-reads the whole held window (a first read sized to the held count, then mention-time pages until covered), and every redraw keeps the reader's place by anchor card (the topmost visible card's rect.top restored within 2 px, the nearest surviving card where the filter took the anchor), through the existing scroll-owner surface only; paging-in keeps its own height anchor. No frozen file touched, no new scroll writer.

Proof: two new fake-DOM unit tests (refresh keeps the paged-in window; clearing a filter keeps the anchor place) fail before and pass after; a new real-server browser walk (`documents_backfilter_browser.py`, 412x915 and 1280x800) stars three documents, grows the fold past the first page while one is open, and asserts Back restores the same cards and place with the count line over the restored window and that clearing Starred keeps the anchor card within 2 px — it times out before the fix and passes after. Dynamic evidence on a test instance (before from main, after from this tree; phone/desktop, light/dark, rect.top sidecars): before, Back read "1 of 100" with the opened card moved 232.3 -> 82.1 and clearing left scrollTop 0; after, Back reads "3 of 120" with the opened card at 232.3 -> 232.3 and clearing keeps the place (scrollTop 150, not the top). The shared screenshot harness matrix (73-76, light/dark x phone/desktop, Discuss through the real route) is green from this tree; the harness fixture seeds no documents, so the targeted states are the real-server walk's shots instead. Unverified: Back by browser gesture (same return path as the button, as in #1336's closure).
