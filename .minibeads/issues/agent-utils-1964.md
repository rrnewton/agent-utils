---
title: 'wrkslots-full-status-hang-ignores-sigterm: full status hangs forever and ignores SIGTERM'
status: closed
closed_at: 2026-11-20T11:20:00-05:00
priority: 0
issue_type: bug
labels:
- wrkslots
- status
- hang
created_at: 2026-11-20T00:00:00-05:00
updated_at: 2026-11-20T00:00:00-05:00
---

# Description

[watermelon-full-preview] Consumer report (devbig014, 9:47 AM ET 2026-11-20; agent-utils 40bf8f75): a FULL `wrkslots status` (no `--slot`) run under `timeout 120` never finished and was still running 44 minutes later. `timeout`'s SIGTERM did not end it. The `python3 -m wrkslots ... status` process had 2 threads, both in futex_wait, plus one DEFUNCT child, and held no file locks. `status --slot <slot>` returns in seconds. It happened during a burst of ~16 lanes running transfer-owner and heartbeats concurrently. The process is gone, so there is no live sample.

# Design

- Find the hang in the full-status path in `py/wrkslots/cli.py`: thread pools, subprocess or census children, `concurrent.futures` and `multiprocessing` waits, any join or wait without a timeout. A defunct child plus two futex-waiting threads suggests a main thread blocked on a join, future or queue whose worker is waiting on a child that already exited, or on a pool shutdown.
- Fix the hang: every wait is bounded or event-driven, with a clear refusal on timeout. Never wait forever on a child or a worker. Reap children.
- Fix SIGTERM: status (and every read-only command) exits promptly on SIGTERM and SIGINT. Install handlers, or make sure no thread blocks signal delivery: Python only runs signal handlers in the main thread, so a main thread stuck in an uninterruptible C-level wait (`lock.acquire()` without a timeout, `Thread.join()`) defers them forever. Use timed waits in loops.
- Check the Rust edition (`rs/wrkslotsd`) for the same full-status path, if it exists.
- No new persisted fields (a consumer pins an older wrkslots reader); report any reader-skew need before landing.

# Acceptance Criteria

- A full status under concurrent mutation (heartbeat / transfer-owner loops mutating state) completes.
- SIGTERM during a full status exits within a bounded time (a liveness assertion, not a performance budget).
- A stuck or killed census child gives a clear refusal, not a hang.
- Tests are robust on a hot host: no wall-clock budgets in gating checks, event-based waits.

# Notes

Reproduce in scratch only (a scratch project with many slots, then a full status while N concurrent heartbeat or transfer-owner loops mutate state; also inject a child that exits early or is killed mid-census). Never touch the consumer's project or real wrkslots state.

# Resolution

[watermelon-full-preview] Closed 2026-11-20, landed as `0d235c2f` on main. Root cause was not a wrkslots status hang: on the consumer's host `python3` is the fbpython launcher, which forks the real interpreter and waits with SIGTERM caught, so `timeout` signaled only the launcher and never the work (confirmed by launcher anatomy — main thread in wait4, second thread in futex_wait, SIGTERM caught — and by the signal topology; 320+ probe cycles did not reproduce the racy deadlock itself). Worker-side audit found every full-status wait bounded (lock 30s, scan deadline 180s, liveness 15s, census deadline with killpg and bounded reap) and no SIGTERM handler, so no genuine status hang exists to fix. Fix: the `common/bin` dispatcher's Python leg resolves the host's real interpreter (safely cached, fully validated) and execs it on the tracked script, so supervisors signal the process doing the work; docs direct callers to `bin/<tool>` under `timeout --kill-after=15` instead of `python3 -m wrkslots`. The init command symlink follow-up is #1965 wrkslots-init-command-wrapper. Gate reds were environment-only (Chromium `libnspr4.so` missing), waived by coordb; combined tree verified (98 passed) via the verified-tree path. No persisted fields changed.
