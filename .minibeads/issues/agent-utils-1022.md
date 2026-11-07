---
title: 'documents-view-client: three-way selector, Documents list and reader draft'
status: open
priority: 1
issue_type: task
external_ref: gh-309
created_at: 2026-11-07T17:47:48.041198347+00:00
updated_at: 2026-11-07T17:47:48.041198347+00:00
---

# Description

GitHub issue #309 documents-view-client (https://github.com/rrnewton/agent-utils/issues/309) tracks the same work for outside readers; this is the local record.

## Description

Owner directive 2026-11-07: build the vibe-talk Documents view now as a usable draft. The server side (#628 documents-index-server: fold index, fetch APIs, list API) is implemented; the client (Phase 1b of ai_docs/transient/2026-10-26-documents-view/README.md) was waiting on #624 voice-page-modularity, and the owner overrides that wait for this lane. Wanted: a three-way view selector (Voice / Channel / Documents) whose button shows the current view and pops two small cards above it naming the other two, a Documents list of linked documents in time order with fetch-on-tap, and a full-screen reader with Read and Back.

## Design

New modules under vibe-talk/web/src/view/documents/ (list, reader, safe escaped Markdown render, selector) with injected dependencies and no new top-level legacy state in voice.js. The only voice.js share is the mount: pressViewSwitch body delegates to the selector, showView gains a docs branch, Ctrl+5 and ?v=docs enter the view, pane-docs in voice.html. Client rendering never passes raw HTML through: escape-first with a strict allowlist (no script/style/iframe/on*, links only http(s) or in-app with rel=noopener). Server endpoints: GET /api/v1/documents, GET /api/v1/documents/{key}, POST /api/v1/documents/fetch, POST /api/v1/documents/fetch-recent, GET /api/v1/documents/{key}/freshness.

## Acceptance Criteria

- Selector rotates Voice -> Channel -> Documents -> Voice, button names the current view, two cards above name the others; Ctrl+5 opens Documents; existing shortcuts keep working.
- List renders server rows in mention order, unfetched dimmed, tap fetches then opens; reader renders safely (malicious-Markdown test renders inert) with Read and Back.
- Focused tests pass; the exact push tree passes validate --all, check_client_names and cargo fmt --check; desktop and phone screenshots reported.

## Notes

[watermelon-full-preview] Filed by mu-docview. Draft lands as its own commit after #624 batch 78 (888124cc); mu-refactor rebases over the mount.
