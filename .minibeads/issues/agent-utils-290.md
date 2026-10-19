---
title: 'speech-ticket-refresh: never play a prepared read-aloud URL after its server ticket has expired'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-14T14:48:59.064823163+00:00
updated_at: 2026-10-19T10:06:02.406936506+00:00
closed_at: 2026-10-19T10:06:02.406936406+00:00
---

# Description

[opus 5.5] Found by the 2026-10-14 branch triage (ai_docs/transient/2026-10-14-branch-triage/REPORT.md). Unlanded work from closed PR #118 speech-ticket-refresh (archived/speech-ticket-refresh and archived/rescue/20260911-pr118-speech-ticket-refresh-c18f3d51), written against the tool's old directory name, so it no longer applies. On main, vibe-talk/web/voice.js writes preparedUntil but never reads it. A row prepared longer ago than the ticket lifetime, or whose ticket the server evicted, keeps a dead URL: a tap plays it, gets a 404, shows 'that message could not be played.', and never falls back. Re-implement in vibe-talk/web/voice.js: store a deadline per entry, {url, until: requestedAt + ttl*1000}, with requestedAt from the monotonic clock taken before the prepare request (per entry, because prepareSpeechFor mints per-row batches at different times; delete the global preparedUntil); treat expired entries as missing in prepareSpeechNow, prepareSpeechFor and readAloud, so readAloud re-prepares once and falls back to the authenticated fetch. vibe-talk application code has its own owning agent.

# Acceptance Criteria

An expired prepared URL is never played; a tap after expiry re-prepares once and falls back to the authenticated fetch if that fails; page tests cover the TTL boundary, expiry, and a failed refresh.
