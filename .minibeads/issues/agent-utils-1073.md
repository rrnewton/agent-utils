---
title: 'documents-in-app-links: a document link in a message opens the reader'
status: closed
priority: 1
issue_type: task
external_ref: gh-314
created_at: 2026-11-08T11:20:01.749539625+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
closed_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

GitHub issue #314 documents-in-app-links tracks this for outside readers.

## Description

Owner reply 220: a recognized document link inside a message should open that document in the in-app Documents reader (fetching if needed) instead of leaving for the source site, like Slack permalinks open in-app.

## Design

The server render marks recognized GitHub document anchors (data-doc, beside the data-msg marking); the page wires them to open the reader, with modifier clicks still going external. Back returns to the Documents list; returning to the exact message is the refinement.

## Acceptance Criteria

Plain click opens the reader in-app; modifier click does not; browser check covers it; gate and land as usual.

## Notes

[watermelon-full-preview] mu-docview. Number confirmed by its issue-only commit on main before citing.

## Resolution

[watermelon-full-preview] Delivered on main by 06fe404083e7c19978bd1ecde12db6ea6ccb2944 (a document link in a message opens the reader). Recognized document anchors carry data-doc from the server render (vibe-talk/src/render.rs) and open the in-app reader on a plain click, while modifier clicks still go external (vibe-talk/web/src/view/shell/navigation.ts). Covered by the render marking test and the browser walk in vibe-talk/tests/documents_browser.py; Back to the exact message followed under #1082 documents-back-chain (closed). Closed during GitHub triage (Request 098) alongside GitHub #314.
