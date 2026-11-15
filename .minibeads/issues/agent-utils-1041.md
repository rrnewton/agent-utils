---
title: 'documents-moved-fallback: find a moved document by file name on a branch 404'
status: closed
priority: 1
issue_type: task
external_ref: gh-312
created_at: 2026-11-08T00:38:00.546193208+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
closed_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

GitHub issue #312 documents-moved-fallback tracks this for outside readers.

## Description

Live check after the proxy deploy: 5 of 12 linked documents in a private repository 404 because the files moved on main (the source repository reorganized its docs directory); old messages keep pointing at the old paths.

## Design

On a branch-ref 404, GithubFetcher fetches git/trees/<ref>?recursive=1 from the API host through the same proxied, token-bearing client, cached per repo/ref about 10 minutes. Exactly one blob with the same file name is fetched from the raw base and marked moved; none or several reads 'not found at <path> (moved or deleted)'.

## Acceptance Criteria

Moved document fetches and shows 'moved to <new path>' on the row and in the reader; missing/ambiguous shows the not-found words; fake-GitHub tests cover all three.

## Notes

[watermelon-full-preview] mu-docview. Number to be confirmed by its issue-only commit on main before citing.

## Resolution

[watermelon-full-preview] Delivered on main by 2e035c9ed6c7d22107b56645706e8a3b5c248ca1 (fetch a moved document from its new home). On a branch-ref 404 the fetcher looks the file name up in the repository tree (vibe-talk/src/documents.rs) and fetches the single match, marking it moved. Fake-GitHub tests in vibe-talk/tests/documents.rs: a_moved_document_fetches_from_its_new_home, a_missing_or_ambiguous_document_says_not_found, and a_commit_ref_never_looks_in_the_tree. Closed during GitHub triage (Request 098) alongside GitHub #312.
