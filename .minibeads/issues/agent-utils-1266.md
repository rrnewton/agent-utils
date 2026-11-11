---
title: 'documents-search: search only fetched documents from the Documents view'
status: open
priority: 1
issue_type: task
assignee: mu-docsearch
labels:
- vibe-talk
- documents
created_at: 2026-11-10T23:15:00-05:00
updated_at: 2026-11-10T23:15:00-05:00
---

# Description

[watermelon-full-preview] Owner reply 354 (2026-11-10 ET): "I would like a
search button that only searches the fetched documents. Reusing our search
UI and mechanism as much as possible."

A search button in the Documents view that searches only FETCHED documents'
text (title plus body), reusing the existing message search UI (bar, chips,
result list, highlight, jump) and its mechanism. Message search is
server-side over the message cache: the shared query language lives in
`vibe-talk/src/search_query.rs` with its page twin in
`web/src/core/search/`, candidates come from the SQLite trigram FTS in
`vibe-talk/src/cache/search.rs`, and answers are served by
`vibe-talk/src/server_search.rs`. Fetched documents do NOT live in that
SQLite cache: they are held by `vibe-talk/src/documents.rs` in a bounded
in-memory cache (`Service::cached`), so they cannot join `message_fts`
(which is content-backed by the `message` table) without fake message rows.

Design (to coordb before code): keep ONE search language and ONE matching
rule. The server gains a documents scope on the same mechanism:
`GET /api/v1/documents/search?q=...` parses with `search_query::parse` and
matches with `search_query::Query::matches` over the folded title plus body
of FETCHED documents only — never unfetched rows, never message text. Each
hit carries the document key/title, byte offsets of the matched terms in the
title and body, and a short excerpt around the first body match. The client
adds a Search button and bar in the Documents list reusing the search UI
shape (field, result list, `<mark>` highlight); a result opens the reader at
the match: the reader highlights every matched term in the rendered body and
scrolls the first match into view.

Pieces, each landing on its own gate: (1) index plus server query with a
documents scope, with tests; (2) the page search button and results reusing
the search UI.

Settled with coordb (2026-11-10 ET): the fetched cache is bounded and in
memory, so an evicted document silently dropping out of search would be a
silent gap. The simpler correct option is taken: search exactly what the
cache holds, and the results state the bound — the answer carries how many
fetched documents were searched (`searched` count), and the page shows
"N fetched documents searched" with the results, so nothing goes missing
silently. Re-reading evicted bodies is not attempted.

Frozen by the parallel modularity lane: `scrollkeep.ts`, `folding.ts`,
`foldkeys.ts`, `keyboard.ts` and the frame/scroller CSS are not edited here.

# Acceptance Criteria

- Server tests: documents scope returns only fetched documents, matches
  title and body, and never returns message text or unfetched documents.
- Browser test at phone (393x852) and desktop (1280x800): search, a result,
  and the reader opens at the highlighted match.
- Focused tests pass; the owner-tier path-selected gate passes.
