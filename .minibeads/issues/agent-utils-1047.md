---
title: 'documents-list-order: newest first, live, with mention jumps and Back'
status: closed
priority: 1
issue_type: task
external_ref: gh-313
created_at: 2026-11-08T03:56:24.148372742+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
closed_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

GitHub issue #313 documents-list-order tracks this for outside readers.

## Description

Owner replies 217-219 (thread 214): the Documents list shows the oldest of the newest 100 at the top with no times, never refreshes, mention counts are dead text, fetched titles can be whole messages from the persisted index, and the Android Back gesture does not return from the reader to the list.

## Design

Client sorts newest-first by last mention with times and scroll paging (before_ms), pull-to-refresh plus live re-checks, mention links with a mentioning-messages pop-up, a render-side title guard, and a document back level through the page navigation. Server-side re-derivation of persisted titles is the follow-up.

## Acceptance Criteria

Newest at top with mentioned-ago times; pull and live refresh; mention jump with Back; reader Back to the list at its scroll place; browser checks including history.back().

## Notes

[watermelon-full-preview] mu-docview. Number confirmed by its issue-only commit on main before citing.

## Resolution

[watermelon-full-preview] Delivered on main by a73cad8afadfbfd8e02e65f59ab3c6e2e0bbe479 (Documents live, with mention jumps and a Back level) and 472d5c003826e6591860f72e05d5e072c6e7f09b (mention-jump Back): mentioned-ago times, scroll paging, drag/live refresh, mention jumps, the render-side title guard, and Back from the reader. Covered by vibe-talk/tests/js/view/documents.test.mjs and the browser walk in vibe-talk/tests/documents_browser.py. Supersession: the newest-at-top order in the acceptance criteria was later deliberately reversed to newest-at-the-bottom by 60e0e36ecacb77bcecb0f907ca68f319f320f85f under #1079 documents-chronological (closed), matching the message views; that decision supersedes the ordering sentence here rather than leaving it outstanding. Closed during GitHub triage (Request 098) alongside GitHub #313.
