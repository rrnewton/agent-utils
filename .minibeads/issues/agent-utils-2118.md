---
title: 'vibe-talk-voice-tools-cache-only: voice-agent read tools must read only the synced cache, never the chat provider'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- voice
created_at: 2026-11-22T11:00:00-05:00
updated_at: 2026-11-22T21:00:00-05:00
---

# Description

The voice tool audit (#2115) found that six voice-agent read tools (recent activity, digest, page reads, counts, find and read message) are cache-first but not cache-only: on a cache miss the server reads the chat provider itself, every cached answer starts a background provider refresh, and with serving off or a read token every voice read becomes a provider read. This contradicts the owner rule that reading should not require upstream operations; the Discuss agent's equivalent tools are already strictly cache-only.

# Design

Give the voice read tools the Discuss cache-only rule: answer from the synced cache, report a miss honestly instead of fetching, start no background refresh from a tool read. A ratchet test calls every listed tool on a cold cache with a provider fake that records calls and fails if any tool reaches the provider; a source guard keeps the MCP layer from using the provider client. No wall-clock assertions.

# Acceptance Criteria

- The ratchet test passes with an empty known-upstream list; the source guard is in place; voice answers on a cold cache say what is missing rather than fetching.

# Notes

Filed by the coordinator from the #2115 audit.

## Fix notes from #2115 vibe-talk-voice-tool-audit (2026-11-22)

The ratchet test and the source guard are on main: `vibe-talk/tests/voice_tool_boundary.rs` (registered in `tests/integration.rs`). Run it with `cd vibe-talk && cargo test --test integration voice_tool_boundary`. What remains for this issue is the fix itself, in this order:

1. **Add a warm-cache case first.** The landed test runs every tool on a COLD cache, so it proves only that the never-upstream tools make no provider call when nothing is held. It does not cover "start no background refresh from a tool read": on a cache HIT, `ops::refresh_behind` (digest, find and read paths) and `recent_activity`'s `recent::refresh_all` / `keep_or_refresh` still start provider reads behind the answer. Add a second test in the same file: build the deployment the same way (serving on, `CountingChat` over the fake), warm the cache for the fixture channel the way `tests/message_cache_serve.rs` does (its `fill_timeline`, `fill_newest` and `fill_window` helpers), advance nothing, then call each of the six read tools for content the cache holds, `drain` after each, and assert zero new provider calls. From the audit, expect it to fail at first for the tools whose cached answer calls `refresh_behind` and for `recent_activity` (not yet run); give it its own known list, which may only shrink, until step 2 empties it.
2. **Make the six read tools cache-only for tool reads** (`ReadReason::Tool`): `recent_activity`, `digest_channel`, `read_page` (the `before`, `until` and resume steps always read upstream today), `count_messages` (the range and walk fallbacks in `ops::count_within`), `find_message` (`ops::resolve`'s `fetch_recent_dated` / `fetch_recent` fallback) and `read_message` (`message_by_id_scoped` and `lookup_messages` after `ops::held_message` misses). Follow the Discuss rule (`ops::catchup_fetch`, #1348): answer from what is held, say plainly what is not held (and how far back the held copy reaches), and leave the filling to the server's own replication. Start no `refresh_behind` from a tool read.
3. **Empty `KNOWN_FALLBACK`.** As each tool's fallback goes, the cold-cache test fails with "move it from KNOWN_FALLBACK to NEVER_UPSTREAM"; move it. When the list is empty, delete the constant and its loop, so the test is the plain rule: no voice tool reaches the provider.
4. **Decide serving off and the read token** (audit finding 3). `ops::may_serve` requires the write scope and `[message_cache] serve = true`; with either missing, every voice read goes upstream today. Once tool reads are cache-only, choose whether a tool read ignores `may_serve` (always the cache) or answers "not held"; either way it must not fall back to the provider. Add that deployment shape to the test.

Keep as they are: `list_channels`, `search` (cache only), `fetch_document` and `fetch_document_section` (fetch nothing), `mark_read` (local dismissals only) and `post_reply` (a proposal under the shipped default; a confirmed send goes through the post gate). No wall-clock assertions anywhere: count calls, wait on conditions.
