---
title: 'documents-fetched-persist: downloaded documents still show as downloaded after a server restart'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-14T06:38:00.000000000+00:00
updated_at: 2026-11-14T06:38:00.000000000+00:00
---

# Description

In the documents view the owner downloaded a few documents, came back later, and they no longer showed as downloaded. The server had restarted (several deploys in one night), and every restart dropped the downloaded state.

# Design

Persist fetched document contents on the vibe-talk server, beside the existing document-index fold (`documents-fetched.json` in the same data directory as `documents-index.json`, derived from `[documents] index_path` and beside `[storage] path` by default). The file holds the bounded cache's copies in cache order under format version 1, keyed by the same document key and stored content version (SHA-256 where the source gives no version) as the in-memory cache. Loading happens on startup under the existing count and byte ceilings (`DEFAULT_MAX_CACHED` / `DEFAULT_MAX_CACHED_BYTES`, or the configured `max_cached_documents` / `max_cached_bytes`); a stored SHA-256 that does not hash its content, an oversized copy and an empty key are dropped, while a source version such as an ETag is kept verbatim. Every fetch writes through atomically (temp file beside the target, then rename), evictions and cache clears leave the same state on disk, and what load drops or evicts is rewritten trimmed. A missing file starts empty, as an old server does; a corrupt, partial or foreign-version file is ignored with one warning and never fails startup. No page change: the list payload's `fetched` flag already comes from this server cache, so it is the downloaded truth after a restart.

# Acceptance Criteria

- Fetch, then a new service on the same directory, still holds the copy and the documents list payload reports it `fetched: true` with its real title and version, with no refetch.
- The count ceiling and the total-bytes ceiling hold in the live cache and across a restart; the oldest copy stays evicted.
- A corrupt or truncated fetched file is ignored (empty cache, no startup failure) and the next fetch replaces it with a whole file.
- Writes are atomic: no temp file remains, the file parses as one whole version-1 record keyed by document key and content version.
- A server with no persisted file starts empty, exactly as before.
- Rust tests cover all of the above with temporary directories only and no wall-clock waits.

# Notes

Owner request 052 (2026-11-14): "In documents view I downloaded a few earlier. Now I come back and they do not show as downloaded."

## Resolution

[watermelon-full-preview] Cause: `vibe-talk/src/documents.rs` kept fetched document contents only in the in-memory bounded cache (`CacheState`); only the mention fold persisted, as `documents-index.json`. Every server restart therefore emptied the cache, the documents list answered `fetched: false` for everything, and the page — which draws its downloaded check from that server flag — showed previously downloaded documents as not downloaded, however many times they had been fetched.

Fix: the fetched cache now persists beside the fold as `documents-fetched.json`. `Fetched` is deserializable; `Service::with_persistence` loads the file on startup under the existing count and byte ceilings, drops a copy whose stored SHA-256 does not hash its content (a source ETag is kept verbatim), and rewrites the file trimmed when load drops or evicts anything. `Service::put` writes through on every fetch (evictions included) and `clear_cache` persists the empty state, both atomically by temp-file-plus-rename with one warning on failure and never a startup failure; a corrupt, partial or foreign-version file is ignored with a warning and starts empty. `service_from_config` derives the path from `index_path` via `fetched_path_for_index`, so it sits beside `[storage] path` by default; with no fold path the cache stays in memory only, as before. No page files changed: `document_row`'s `fetched` flag is this cache, so the downloaded state now survives a restart by construction.

Tests: integration tests in `vibe-talk/tests/documents.rs` fetch through a fake fetcher, restart as a new service on the same temporary directory with an empty fetcher, and assert the copy and the `GET /documents` downloaded row survive; count and byte ceilings hold across restarts; corrupt and truncated files are ignored and then replaced by a fetch; and the written file is whole, version 1, keyed by key and SHA-256 version with no temp file left. Unit tests in `documents.rs` pin that ETags survive, SHA mismatches and empty keys drop, and the persisted path sits beside the fold.
