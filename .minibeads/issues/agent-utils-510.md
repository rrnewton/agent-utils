---
title: 'cache-serve-every-read: a served timeline page blocks the next serve of its view, so only half of servable reads are served'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- message-cache
created_at: 2026-10-25T15:07:48.839759861+00:00
updated_at: 2026-10-25T15:55:55.062215298+00:00
closed_at: 2026-10-25T15:55:55.062215037+00:00
---

# Description

[opus 5.5] On the Slack instance (build b6dfeed1), the hour ending 14:35Z: timeline_fallback asked=144 would_serve=100 served=51; read timings cache n=51 p50 0.7 ms p90 0.8 ms p99 2.2 ms, upstream n=93 p50 4424 ms p90 5571 ms p99 7307 ms, would n=49 (servable, went upstream). Misses gap=43 suspect=1. The hour before: 50 would_serve, 22 served (11 in the earlier figure quoted).

Cause (sink.rs try_serve, cursor_owed): a served newest page carries no backward cursor, so the view is marked as owing the provider's own page and is not served again until a reader is handed one (ops::timeline note_real_page). Reads of a view therefore alternate: served, upstream, served. That keeps older history reachable, since the page's first step back from a cursorless page is itself a newest read, which would otherwise be served again with no cursor.

Separately, 43 gap misses: a row pushed live since the last newest read sits above the head that read listed and is held only as the push brought it (merge rule 5), and either makes a newest page a gap until the next relist (every 90 s, 40 rows against the page's 50).

# Acceptance Criteria

Back-to-back newest reads of a view are served whenever the rule allows; a step back from a served page still reaches older history (also after N served reads), the refresh's corrections are still applied, account checks, read scope and the shadow comparison unchanged. Tests show each. README cache section updated. The gap misses are broken down by cause in the hourly figures and diagnostics, with a recommendation on whether pushed rows could vouch for the head.
