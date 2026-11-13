---
title: "rust-writer-turnstile: Rust wrkslots writers take the intent-lock turnstile while waiting for the registry lock"
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-10T10:00:00.000000000+00:00
updated_at: 2026-11-10T10:00:00.000000000+00:00
---

# Description

#1172 audit-lock-hold gave the Python edition a writer turnstile: a writer waiting for `ACTIVE.global.lock` holds
`ACTIVE.global.intent.lock` exclusively, and new readers queue behind it. Measured on a 120-slot fixture, a
`hold --wait-lock 300` behind looping status, doctor and audit waited 134 s before and 3.3 s after. The Rust edition's
writers (`acquire_mutation_locks` in `rs/wrkslotsd/src/hold_write.rs`, `acquire_lock`/`flock_wait` in
`rs/wrkslotsd/src/ensure_states.rs`, used by create, adopt, release, transfer-owner, heartbeat, hold, handoff and
recover-*) still poll `ACTIVE.global.lock` with LOCK_NB every 50 ms without taking the intent lock. So Python readers
don't queue behind a waiting Rust writer, and overlapping readers can still starve it. On a host that runs both
editions, most mutations go through Rust.

# Design

While polling for `ACTIVE.global.lock`, a Rust writer holds `ACTIVE.global.intent.lock` exclusively (flock on the fd,
as Python does) and closes it once the registry lock is acquired. A stale intent file must block nobody. Rust readers
take no registry lock, so they need no change. Mixed versions stay deadlock-free; keep the old-style-reader test.

# Acceptance Criteria

- A differential: a Rust writer waiting behind a held reader makes new Python readers queue behind it, and it
  acquires within a bounded time once the first reader releases.
- The writer-wait measurement from #1172 (fixture and tool in the #1172 handoff), run with a Rust writer: before
  and after numbers recorded in Notes.
- Mixed Python/Rust readers and writers never deadlock (existing and new tests).

# Notes

Filed for the lead coordinator (2026-11-10) from #1172's Rust parity report. Planned for the Rust wrkslots lane after
its scoped-readers soak opt-in lands.

## Resolution

[muse watermelon] Done. `ensure_states.rs` now implements the
writer-intent turnstile: `acquire_lock` (the exclusive registry
lock) first takes `ACTIVE.global.intent.lock` exclusively through
`enter_turnstile` and holds it while polling, closing it once the
registry lock is acquired; a second writer reports "another
writer is waiting for it". `acquire_shared_lock` (the #1200
readers' last-attempt lock) passes through the turnstile shared,
deferring at most 10 s like Python `_REGISTRY_INTENT_DEFER_SECONDS`,
then proceeds. Held locks are tracked per thread, so a nested
acquisition skips the turnstile and cannot deadlock against a
writer waiting for that same thread. Only the flock counts, so a
stale intent file blocks nobody, and callers that never heard of
the file weaken fairness but cannot deadlock.

Proof: crate tests `waiting_writer_holds_the_intent_lock`,
`stale_intent_file_blocks_nobody`, and
`second_writer_reports_another_writer_waiting`; the cross-edition
differential `test_lock_turnstile_rust.py` holds the registry
lock shared from Python, shows the Rust `hold` writer waiting
with the intent lock exclusively held and a new Python reader
queued behind it, then acquiring within 5 s of release. The
#1172 writer-wait fixture/tool lives on devbig030; the bounded
acquire here is the same shape (writer completes as soon as the
readers inside release, instead of polling behind overlapping
readers indefinitely).
