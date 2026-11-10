---
title: 'proccpu-thread-cpu-bound: bound the procfs CPU scan by the scanning thread''s CPU time, with a wall backstop'
status: closed
priority: 1
issue_type: bug
assignee: cl-proccpu
created_at: 2026-11-10T19:16:27+00:00
updated_at: 2026-11-10T20:30:00+00:00
closed_at: 2026-11-10T20:30:00+00:00
---

# Description

[opus 5.5] dagrun's process-group CPU scan (`ProcessGroupCpu::seconds` in rs/dagrun/src/proccpu.rs, and py/dagrun/proccpu.py) walks every PID in /proc and is bounded by 1 s of WALL time: a per-entry deadline check and a final span refusal. On a loaded host the scanning thread is runnable but not scheduled for most of that second, so the scan is refused with "scan deadline" after doing only a fraction of its work. The refusal makes dagrun's scheduler fall back, and makes consumers' live CPU meters and their tests flaky under load.

Measured by a consumer (temporary probes, then reverted): alone, a scan did about 4,460 stat reads in 135-140 ms of wall time and the same thread CPU time (about 31 µs per read, so the work is CPU-bound). Under a full test run at 1-minute load 50-236, scans took 308-1,020 ms of wall time but only 195-345 ms of thread CPU; the thread was off-CPU for 65-78% of the wall time, and 2 of 10 scans were refused after 224 ms and 345 ms of real work. Registry lock waits were all under 10 ms, so dagrun's own locking is not the cause.

A consumer prepared a fix as three patches on 28c95330 (tests first, then the Rust bound via `CLOCK_THREAD_CPUTIME_ID`, then the Python bound via `time.thread_time`), touching only proccpu.rs, proccpu.py and py/tests/test_proccpu.py.

# Design

- Bound the scan by the scanning thread's own CPU time (the snapshot runs synchronously on the thread that holds the registry lock, so its thread CPU clock counts exactly the scan's work).
- Keep a generous WALL backstop as well (for example 10x the CPU budget, or 10 s), because a thread-CPU bound never fires if the scanner blocks in a syscall without burning CPU: a /proc read of a D-state task, or a stalled FUSE mount. A scan must never hang unbounded.
- Rust and Python change together, with the same budgets and refusal texts.
- Tests: a scan that survives an off-CPU stall (the original failure); a scan whose CPU budget is exceeded is refused; a scan blocked off-CPU past the wall backstop is refused; both editions.

# Acceptance Criteria

- Under load, a scan with CPU work inside the budget is no longer refused, however long it waits to be scheduled.
- A blocked scan is refused by the wall backstop; a CPU-bound runaway scan is refused by the CPU bound.
- Python and Rust parity in the same change; full validation passes.
- Lands before #1242 testrun-core-crate moves proccpu (same owner).

# Notes

Handed over by a consumer coordinator with patches (2026-11-10); commit messages are rewritten neutrally and authored by this repository's bot. Lead review point (wall backstop) added 2026-11-10.

## Resolution

[opus 5.5] Delivered in the commit that closes this issue, from a consumer's three patches (reviewed, rewritten neutrally) plus the wall backstop.

- Both editions bound a scan by two budgets: 1 s of the scanning thread's CPU time (`CLOCK_THREAD_CPUTIME_ID`; `time.thread_time`), refused "scan deadline" (text unchanged, so callers need no change), and a 10 s wall backstop, refused "scan wall deadline". `check`/`_check` tests both between steps, and a finished snapshot is refused if its CPU time exceeds 1 s or its wall time exceeds 10 s. The 500 ms freshness window stays wall time. An unreadable thread clock fails closed.
- Limit, by design (lead accepted): both budgets are checked between system calls. The backstop bounds blocking that accumulates across steps; no in-thread check can interrupt ONE call that never returns (a D-state read). The old wall bound did not cover that case either, so this is no regression.
- Evidence from the consumer's report: alone, a scan did about 4,460 stat reads in 135-140 ms of wall time and the same CPU time. In a loaded test run (1-minute load 50-236), 10 scans took 308-1,020 ms of wall time but only 195-345 ms of CPU, and 2 were refused by the old wall bound after 224 and 345 ms of CPU. Registry lock waits stayed under 10 ms.
- Tests, the same in both editions: an off-CPU stall longer than the CPU budget still yields a reading (it failed on the old bound: Rust 5/5, Python); a CPU burner is refused "scan deadline"; a blocked scan under a 200 ms wall budget is refused "scan wall deadline"; crafted snapshots over either budget are refused. Mutants killed in both editions: wall check removed from `check`, snapshot wall-span check removed, CPU budget moved back onto wall time, CPU budget never firing.
- Python caveat: a cold Python census costs 590-861 ms of CPU on a busy host, close to the 1 s budget. The tests' `_seconds()` retry of "scan deadline" (#389 gate-flakes) is kept for that reason.
