---
title: 'vibe-talk-documents-keyed-cards: reuse document cards across refreshes instead of rebuilding every card'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-17T22:05:00-05:00
updated_at: 2026-11-17T22:05:00-05:00
---

# Description

[watermelon-full-preview] The Documents list rebuilds every card on every render: `render()` in `vibe-talk/web/src/view/documents/list.ts` maps all visible rows through `cardFor` and swaps them in with `list.replaceChildren(head, ...cards, ...)` (the empty and filtered branches rebuild the same way). A live refresh therefore discards every card's DOM identity even when nothing about that card changed.

That has reader-visible costs, not just test costs:

- Focus is lost: the list's arrow-key handler finds the focused card with `document.activeElement` among `.doc-card` elements; after a rebuild the focused node is gone and focus falls back, so keyboard walking breaks on any refresh.
- Any in-flight interaction loses its node (a tap target, a scroll anchor, an open swipe), because the element it started on no longer exists.
- Every refresh pays full card construction and layout for the whole list.

Measured while making `tests/documents_browser.py` robust under load (the #1765 step-3 follow-up work): a locator scroll into view died with "Element is not attached to the DOM" and a star read timed out resolving its locator, both because a refresh landed between resolving a card and acting on it. The test now queries, scrolls and measures inside one page task; keyed reuse would remove that whole class at the source.

# Design

- Keep a card element per document key across renders: build once, update in place (title, meta, star, hidden, fetch state), and reorder by moving existing nodes instead of recreating them.
- Preserve the current rendering contract exactly: same order (oldest first, deduped), same buckets/starred/hidden filtering, same head/undo/bucket bar, and the same classes and data attributes the tests and styles read.
- Focus, an in-progress swipe and the scroll position of an unchanged card survive a refresh; a card whose document changed updates its contents without losing identity.
- Where a row's kind changes what the card contains structurally, rebuild that one card only.

# Acceptance Criteria

- Two renders of the same rows keep the same card elements (identity observable by key), with contents updated in place.
- A focused card keeps focus across a refresh, and arrow-key walking continues from it.
- Filtering, starring, hiding/unhiding, the undo chip and the bucket bar behave as before; the documents browser walks (list, buckets, layout, control bar, documents) pass unchanged.
- A refresh with one changed document does not recreate the other cards.

# Notes

Related: #1263 documents-list-cards, #1265 documents-star-hide, #1306 doc-star-gold, #1765 wrkslots-load-sensitive-gate-tests (the load work that measured the detach failures). Filed from the follow-up queue; the test-side robustness that works around the rebuild today is deliberate and stays either way.
