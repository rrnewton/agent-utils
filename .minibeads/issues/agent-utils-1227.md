---
title: 'poll-storm-stale-thread: a failed cache refresh re-polls in 3 s and starves the upstream route it waits on'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- cache
created_at: 2026-11-10T17:34:00+00:00
updated_at: 2026-11-11T00:12:00+00:00
closed_at: 2026-11-11T00:12:00+00:00
---

# Description

[watermelon-full-preview] Owner report, 2026-11-10 11:39 AM ET: a thread reply and its reactions posted minutes earlier did not show in vibe-talk until a page refresh, though the provider's own client had them at once.

## What happened (server log, 2026-11-10, times ET)

- From 11:00 AM ET the page's poll rate jumped about 20x: `read_reason=poll` lines per 10 minutes went from 10-23 to 263/351/375/232 and stayed elevated (209/169) until mitigation. Two channels were polled every ~3 s, each poll with a side thread read.
- Those thread reads exhausted the provider's `conversations.replies` rate bucket (rate-limit warnings from 11:32 AM ET; side reads answered 502). The read behind each cache answer kept failing (335 such failures in 11:30-11:39 AM ET, mostly `view=Thread`), so the thread view kept serving the stale copy.
- The live event path did not save it: from 11:05 AM ET live ingestion was itself rate-limited on all channels (`conversations.history`), backing off without advancing its cursor, and a thread reply still needs a replies read to materialise. The reply reached the page only on the 11:39 AM ET refresh's `enter` read, served from upstream.
- Mitigation, 12:19 PM ET: `[message_cache] serve = false` plus a restart. Polls fell to 38 then 15 per 10 minutes, behind-read failures went to zero, live-ingestion rate limits stopped. The cost is the pre-cache-first behaviour: reads wait on the provider again.

## Root cause

`#622 cache-first-reads` couples the page's read rate to the server's upstream read rate:

- Every eligible page read answered from the cache sets `refresh_failed` (`vibe-talk/src/ops.rs`, `CACHE_FIRST`) and starts one upstream read behind the answer (`read_or_stand_in` -> `spawn_behind_cache_first`, single-flight per view).
- The page (`vibe-talk/web/src/core/sync/poll.ts`) answers a `refresh_failed` read with quick retries (`STAND_IN_RETRY_MS`, 3 s then 10 s) instead of the normal 45 s poll. Every cache-first answer counts as failed, and a landed or current read resets the retry budget, so the retries never settle while refreshes keep failing.
- `behind_read_wanted` returns true unconditionally for Thread and Threads views, so every side poll starts a fresh replies read behind its answer. While that route is rate-limited every behind read fails, `refresh_failed` stays set, the page re-polls in 3 s, and the loop keeps the bucket empty. It tipped over once the replies bucket first saturated (thread naming and All reads were already hitting it minutes before the storm); neither deploy that morning caused it.

The browser never calls the provider. The violation is the coupling: a page read must never be what starts an upstream read.

## Fix

Decouple page reads from upstream refresh, server-side:

1. The server schedules upstream refreshes itself: per-view freshness, a rate budget per provider route, and back-off on rate-limit answers. Page reads never trigger an upstream read.
2. A failed refresh retries on the server's own back-off and marks the view visibly stale (the answer still says how old the copy is); it recovers by itself when upstream comes back. `refresh_failed` never shortens the page's poll below the normal cadence.
3. Thread reads are budgeted: an unchanged side thread is not re-fetched on every poll.
4. The live stream path delivers thread replies and reactions, so a poll is only the fallback it was meant to be.

## Tests

Against a fake provider:

- A rate-limited replies route: page polls stay at the normal cadence, upstream replies reads back off, the view shows stale, and it recovers without a reload when the limit lifts.
- A dropped live stream: the bounded poll still bounds upstream reads.
- A thread reply plus a reaction arriving over the live stream reaches the page with no poll in between.

# Acceptance Criteria

- With the provider rate-limiting thread replies, the page poll never goes below the normal cadence and upstream reads of a view are server-scheduled and budgeted per route.
- A thread view whose refresh fails shows its staleness and recovers on its own when upstream recovers; no reload needed.
- Thread replies and reactions delivered by the live stream appear without waiting for a poll.
- The deployed instance's `[message_cache] serve` is restored to `true` once this lands, and the log shows normal poll cadence with no behind-read failure loop.
- Focused tests above pass; the owner-tier gate passes with path selection over every touched area, plus a full-validation follow-up after landing.

# Notes

- Mitigation in place since 2026-11-10 12:19 PM ET: `serve = false` (backup kept beside the config). Restore is part of acceptance, not a follow-up favour.
- The provider shim (a deployed program outside this tree) logged `BrokenPipeError` tracebacks when the app closed a connection before a slow replies call answered, and counted the call `internal_error`. With serving restored this fix removes the pattern from the read path: cache-held views never wait on the shim, and refresh reads run detached to completion on their own connection. A cache-miss read still waits upstream, bounded by the existing stand-in waits. Quietly logging a client-gone write in the shim itself is a deployment-side change, noted here and not part of this fix.
- Cosmetic, unrelated, no fix wanted here: one settled send can log `send_timing` twice (the posting task logs it, and the two-phase send's detached task logs it again after its own settle note). One upstream post either way.
- Coordinates with the lanes holding the page trace/selection work, the redraw/reads work, and the voice-page code move: file map shared before editing shared files.

## Resolution

[watermelon-full-preview] Landed on `github/main` as 75671087 (gate green on the path-selected run; issue filed as b090972b). Deployed 2026-11-10 5:37 PM ET and `[message_cache] serve` restored to `true` at 5:38 PM ET. Live verification from the app log: polls bounded at the normal cadence per channel (13-21 per 10 minutes across four channels, against 232-375 on two channels in the storm), behind-read failures down about 25x and only on the server schedule with back-off, no 3 s loop, live ingestion recovering between shared-bucket rate-limit bursts.

The `--all` follow-up finished red on seven steps in areas this change does not touch. Five pass alone locally (keyboard-browser, chat-subscription fake, fold-latency, dagrun, wrkslots-debug; exit 0 each). The two lifecycle host shards are pre-existing and load-sensitive: they fail on `github/main` under load, are untouched here, and are being quarantined separately (wrkslots-load-robust). Shard-0 failed alone at its time cap and shard-2 was skipped on that basis.
