---
title: 'documents-moved-fallback: find a moved document by file name on a branch 404'
status: open
priority: 1
issue_type: task
external_ref: gh-312
created_at: 2026-11-08T00:38:00.546193208+00:00
updated_at: 2026-11-08T00:38:00.546193208+00:00
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
