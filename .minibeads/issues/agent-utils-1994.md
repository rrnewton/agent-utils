---
title: 'documents-list-meta-separators: Documents list meta line shows stray separators when fields are empty'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-20T15:24:00-05:00
updated_at: 2026-11-20T15:24:00-05:00
---

# Description

[watermelon-full-preview] Spotted by coordb on 2026-11-20 while reviewing the #1989 failure-state screenshots (light, phone-412, broken-row shot): in the Documents list, a card's meta line starts with stray separators when its fields are empty — seen as "·· fetched just now·0 mentions" and "p.· ·0 mentions". In `vibe-talk/web/src/view/documents/list.ts` the meta line appends `` ` · ${mentionedAgo(...)}` `` and a bare `" · "` before the mentions count unconditionally, so an empty mentioned-ago (or another empty leading field) leaves its separator behind with nothing before it.

# Design

- Build the meta line from its non-empty parts only: join the parts that have text with " · ", never emitting a separator for an absent field (file name, mentioned-ago, issue status, fetched-ago, mentions, kind).
- Keep the failed-card and moved-card meta lines as they are; they do not build from parts.
- Cover it with a page test: a row whose mentioned-ago (and any other field) is empty draws no leading or doubled separator, and a fully populated row keeps its current wording and order.

# Acceptance Criteria

- A Documents card with an empty mentioned-ago shows no leading "·" and no doubled "··" in its meta line, at phone and desktop sizes.
- A fully populated card's meta line is unchanged: file name, mentioned-ago, fetched-ago, mentions and kind in the same order with single " · " separators.
- The existing documents list/browser tests still pass, with a new assertion for the empty-field row.

# Notes

[watermelon-full-preview] Filed by mu-docshot at coordb's assignment on 2026-11-20; number assigned by coordb, not picked by the lane. Not a #1989 regression — the meta line predates it (#1263 documents-list-cards) and #1989 touched no list code; it was only photographed there. Per coordb: fix AFTER #1989 lands, as its own commit, with a page test.
