---
title: 'cache-first-reads: every page read and read-aloud answered from the server''s message cache, Slack behind the answer'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- cache
created_at: 2026-10-28T12:39:00.591856816+00:00
updated_at: 2026-10-28T13:34:10.204021567+00:00
closed_at: 2026-10-28T13:34:10.204021417+00:00
---

# Description

[opus 5.5] The owner, 2026-10-28 08:32 ET: "I thought we were fully cached on the server and at least cached for the recent messages so that it would just be a round trip between my client and our own vibe talk server that I'd be waiting on before messages appear on my screen." And 08:36 ET: "I expect that there's really no situation where my client directly cares about upstream slack at all. I just get a fast view of the server. The server worries about pulling eagerly or lazily information from upstream."

## Today
- Timeline reads: with `[message_cache] serve = true` the strict serve rule answers Main (when fresh) at once, and a live-fed channel's side thread list. Everything else goes upstream first and the cache stands in only after a wait: 1.5 s for a page entering a view (#617 cold-load-first-paint), `timeline_fallback_ms` (10 s) for every other read. Slack's All is never served by the strict rule.
- Read aloud: `POST /api/v1/channels/{id}/speech/prepare` takes a message from the 30-second read-ahead memory or fetches it from Slack. Live 2026-10-28 08:18 ET: `read-aloud preparation ids=1 threaded=true recent_hits=0 fetches=1 fetch_ms=9044 preparation_ms=9045`: 9 s from tap to speech, all Slack fetch; TTS first audio 0.1 s.

## Acceptance
1. Every page read the cache holds the view for is answered from it at once, with one upstream read behind the answer (single-flight); Slack goes first only for what the cache has never seen, with the existing fallback.
2. speech/prepare reads from the message cache first, then read-ahead memory, then Slack. Tap to first audio under 1 s for a cached message.
3. Edits and deletes still arrive; the #617 Refresh-failed pill semantics, the Slack budget and the #614 single-flight rules hold.
4. app.log says per read whether it was served from the cache or upstream, and how long it took; the speech line counts cache hits.
5. Tests with a fake provider: cached All, enter and speech never touch Slack before answering; a miss does.
