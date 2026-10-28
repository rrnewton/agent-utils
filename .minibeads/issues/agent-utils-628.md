---
title: 'documents-index-server: fold index, fetch-on-demand and list APIs for linked documents'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- documents
created_at: 2026-10-28T13:42:29.697292909+00:00
updated_at: 2026-10-28T15:00:48.351756464+00:00
closed_at: 2026-10-28T15:00:48.351756274+00:00
---

# Description

[muse watermelon] Server side of #544 documents-view-design as decided 2026-10-28 (ai_docs/transient/2026-10-26-documents-view/README.md). Start now; no page changes beyond what an API test needs.

Build:
- documents::recognize + canonical document key: GitHub Markdown (blob/*.md, *.markdown, raw.githubusercontent.com), other GitHub files, pastes via the configured command source; kind field extensible (Google Docs later). Strict parsing as link_labels: no decoding, no dot segments; Slack file-name links never match.
- The document index as a PURE FOLD over all messages in the cache, maintained incrementally as messages are cached (live, backfill, edit, delete), persisted or rebuilt from the cache at startup, never a linear scan on the read path. It caches document key -> mentions (when/where), NOT contents. Lazy registration: existence only, no eager fetch. Read-scope reads fold nothing in.
- Fetch-on-demand API: one document, and recent N (most recent 10-20 unfetched); a bounded server document cache (entries+bytes); a freshness-check API (cached version vs source, update_available); the list API (mention order default, fetched/unfetched state, metadata incl. real title once fetched).
- Config: [documents] github token file (least-privilege), paste command source, cache bounds. No per-document privacy gating in the MVP.
- Tests with a fake provider and fake GitHub: recognizer (incl. .. escape, slash branch, >16KiB message), fold edit/delete, bounded cache eviction, fetch one/recent, freshness, list order and auth scopes. Register new tests/*.rs in tests/integration.rs (autotests=false, #569).

# Acceptance Criteria

Fold index updates on cache live/backfill/edit/delete and rebuilds at startup; list API serves from the fold in mention order with dimmed (unfetched) vs fetched state; fetch one and fetch-recent populate the bounded cache; freshness reports a newer version without swapping content; cargo tests green and make validate green.
