---
title: 'wrkslots-wait-lock-covers-busy-finish-journal: --wait-lock waits for the registry lock but not for a live finish journal'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T15:45:41-05:00
updated_at: 2026-11-17T04:33:45-05:00
closed_at: 2026-11-17T04:33:45-05:00
---

# Description

[watermelon-full-preview] Field report (2026-11-16, from an owner-ordered cleanup): only one remove per host can be in its finish step (the one `ACTIVE.<machine>.journal`), and every other remove refuses: "BUSY: live 'finish' mutation ACTIVE.<machine>.journal by pid <n>; retry", exit 75, state UNKNOWN. `--wait-lock` covers the registry lock but not the journal. In 15 minutes: 0 removed, 5 refused, each losing 1-7 minutes of lock-free preparation (validation, census, salvage) that it then has to redo from the top.

The refusal is the `_JournalBusy` path: a live non-create journal's owner makes the guarded attempt retryable-busy, but no caller waits for the journal itself -- prevalidated mutations wait only for the registry locks, and the busy answer discards the preparation.

# Design

- Make `--wait-lock` also wait on a LIVE finish journal: bounded by the same `--wait-lock` budget, event-driven (a flock on the journal or its lock file, or inotify on its removal; no sleep-poll), then re-check from the top so the waited-out removal revalidates on current state.
- When the bound expires, refuse exactly as today (exit 75, BUSY), naming the holder pid.
- A journal whose owner is proven dead is still a `recover` case, unchanged: never waited out, never silently cleared.
- The wait releases every lock the waiter holds, exactly as the existing journal-busy wait in `_run_prevalidated_mutation` does, so the holder can finish.

# Acceptance Criteria

- A deterministic test: a second remove waits while a stub finish holds the journal, then proceeds when the journal is released; its preparation is not redone from scratch (or is redone only by the ordinary revalidation round, as the test shows).
- A deterministic test: with a tiny bound the second remove refuses as today, naming the holder pid.
- Waiting is event-driven (flock or inotify), with no sleep-poll loop; the bound is the caller's `--wait-lock` value only.
- Python and Rust editions agree where both have the path.

# Notes

Related: #1735 wrkslots-remove-holds-state-lock-through-salvage (long finishes are what make the wait matter), #1736 wrkslots-live-seal-reported-as-interrupted (the same live-versus-interrupted distinction for seals).

## Resolution

[watermelon-full-preview] Landed on main as 809b4959: remove and prevalidated mutations now wait on a live finish journal under the same --wait-lock budget, event-driven via inotify/pidfd with no sleep poll and no registry lock held, then re-check from the top; an expired bound refuses busy naming the holder, and a dead owner still refuses as a recover case. Python and Rust parity.
