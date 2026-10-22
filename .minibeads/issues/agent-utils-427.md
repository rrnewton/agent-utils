---
title: 'server-thread-hues: thread colours are decided by the server as messages arrive'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- ui
created_at: 2026-10-22T14:31:16.746579915+00:00
updated_at: 2026-10-22T17:01:01.016139447+00:00
closed_at: 2026-10-22T17:01:01.016139307+00:00
---

# Description

[muse watermelon] Today thread colours are chosen in the browser (web/voice.js threadHue): #396 thread-color-memory remembers them per browser in localStorage, and #414 thread-hue-spread keeps a new thread's hue at least 45 degrees from threads within 3 places and 90 degrees from the adjacent thread, re-rolling deterministically. Two devices can colour the same thread differently. The owner, 2026-10-22: thread colors should be server side -- whatever our policy is we should decide it as messages arrive on the server.

# Design

The server assigns each thread's hue once, when it first learns of the thread (ingestion, a write-scope read, a live update), using the #414 policy: neighbours by root-message time within the channel, the last few assigned, 45/90 degree minimums, deterministic re-roll. Stored durably in the existing store, bounded per channel and erased by the purge. Every message's thread record, thread card and thread summary carries hue; the page uses the server hue whenever present and falls back to the local algorithm only for uncoloured threads (older servers, read-token pages, no store). A browser's localStorage hues may seed the server once through a write-scope call so today's colours survive; a thread never changes colour once assigned.

# Acceptance Criteria

Rust tests for assignment, persistence across restart, the neighbour rule and determinism; page tests for using server hues and the fallback; a two-browser-context Chromium check that both contexts draw identical colours. Same colours on every device and on reload. make validate passes.
