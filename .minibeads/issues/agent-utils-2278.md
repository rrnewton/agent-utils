---
title: 'vibe-talk-all-read-replies-budget-refusal: a budget refusal inside an All read drops threads replies with no server-scheduled retry'
status: open
priority: 0
issue_type: bug
assignee: mu-statesync
labels:
- vibe-talk
created_at: 2026-11-26T04:04:00-05:00
updated_at: 2026-11-26T04:04:00-05:00
---

# Description

Server side of the 2026-11-26 night report (owner requests 379 and 381). At 2026-11-26 3:27 AM ET an All read gave up on 25 threads' replies when the server's own budget refused. Every page request that night was still answered quickly from the server; the harm is that the server's copy then lags with no scheduled catch-up.

Mechanism, from the #2280 survey at `6e12c6c6`: All assembly (`vibe-talk/src/slack/timeline.rs:798 flat`) reads one history page, then per-thread replies reads. Threads that are free from the server copy cost no call; costly threads are capped per read (3 for poll reads, 8 for refresh, 10 otherwise). The per-method budget check (`vibe-talk/src/slack/client.rs:527 admit`) can refuse without sending. Inside `flat`, a rate-limit failure — including a never-sent refusal — marks the read stopped, so every remaining costly thread is skipped. `flat` still returns a successful page (with a notice that some replies are missing), so the outer retry machinery for refused reads never fires, and skipped or failed threads enqueue nothing. Dropped replies are re-read only by a later All read, a scheduled refresh, the live poller or a relist, once earlier threads have become free: dropped work waits for the next read instead of retrying on the server's schedule.

# Design

1. Dropped replies work becomes server-scheduled: when an All read skips or fails costly threads for budget or rate-limit reasons, the server records the owed work and retries it in background under the same per-route budget and back-off, publishing arrivals by live event. A later page read is never the sole retry.
2. The All answer stays honest while work is owed: missing replies are still reported as missing (the existing notice and staleness), and the retry respects the costly-thread caps and provider budgets so it cannot recreate a poll storm (`#1227 poll-storm-stale-thread`).
3. Land with the #2280 server-timer retry step (one owner, one retry design): the refresh schedule gains a server timer for owed work generally, and this issue is its All-read case. No second, competing retry loop.

# Acceptance Criteria

- Reproduced through the real server router with a fake provider that refuses replies reads on budget: the All answer reports the missing replies, and the owed replies then arrive by background retry and live event with no further page read.
- Retries respect the per-route budget and back-off (call counts asserted against a ceiling, never wall time), and stop when the threads become free or held whole.
- The gating coverage for this path is event- or condition-based, has no wall-clock budget, and passes at hot load; it is exercised end-to-end through the real server (stub only the provider) before landing, with evidence.

# Notes

Owning lane: mu-statesync (single owner with `#2280 vibe-talk-state-sync-audit`; this fix is that issue's server-timer retry step). Related: `#2280` (state-sync audit and gating test), `#1227 poll-storm-stale-thread` (page rate must never set upstream rate), `#2277 vibe-talk-key-scroll-freezes-11-26` (client side of the same night). No code until the coordinator approves the #2280 step order.
