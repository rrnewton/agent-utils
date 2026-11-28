---
title: 'python-wrapper-post-fork-hang: a long-lived Python wrapper (with-private-tmp) can hang after its child exits, at zero CPU with SIGTERM blocked, holding a gate step until its wall timeout'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- validation
- gate
- hot-host
created_at: 2026-11-28T14:14:00-05:00
updated_at: 2026-11-28T14:14:00-05:00
---

# Description

On some hosts `python3` is a build whose allocator runs a jemalloc background thread. Under such an interpreter, a long-lived Python wrapper that starts child processes was seen hung, twice on 2026-11-28:
- `scripts/with-private-tmp --host-temp ...`, run as `python3`, at load1 above 230. Its command (a namespaced pytest) had passed and exited, but the wrapper never returned.
- An operator's own loop script (a `subprocess.run` every 30 s), under the same interpreter.

Both hangs had the same signature:
- the main thread blocked in `__futex_wait`;
- a second thread, the allocator's `jemalloc_bg_thd`, also in `__futex_wait`;
- the exited child a zombie, never reaped;
- SIGTERM and SIGCHLD pending but blocked in the main thread (`SigBlk` 0x24000), so a `timeout` around the wrapper could not end it. Only SIGKILL did.

The blocked signal mask and the unreaped child point to a hang inside, or right after, a fork in the wrapper. A plausible cause: an allocator lock held by its background thread at fork time and taken again in the post-fork path. This is not proven.

Why it matters: every gate step runs under `with-private-tmp`, and many under `scripts/case_timings.py`, both as `python3`. A wrapper hung this way:
- holds its step until the step's wall timeout, often hours;
- uses about 0 CPU, so the step's CPU timeout never fires;
- ignores SIGTERM.

dagrun's slow-step reports show python wrappers as "wall-stalled ... cpu=0.0Xs". Whether those are this hang is to be checked.

# Design

- (a) Repro: a `with-private-tmp` loop under process churn and host load. Capture the hung parent by exact PID: its Python and native stacks (py-spy `--nonblocking`, or gdb `thread apply all bt`), `/proc/<pid>/status` (SigBlk, SigPnd, ShdPnd), and its threads. That gives the before rate.
- (b) Check dagrun's "wall-stalled cpu=0.0Xs" lines in recent kept logs against the same wrapper and signature.
- (c) Mitigate in code, not host configuration.
  - `with-private-tmp` starts its child without fork from a threaded process: `posix_spawn` (`os.posix_spawn`, or `subprocess` with arguments that allow its spawn path), or it forks before any thread exists.
  - Then the same for every other long-lived wrapper that starts children (`case_timings.py`, and any found by a sweep).
  - No wrapper may block SIGTERM over a wait that can last.
- (d) Evidence: 0 hangs in at least 100 churn runs after, against the before rate, with load1 recorded per run.
- No wall-clock budget (owner rule 346).

# Acceptance Criteria

- The hang is reproduced, and its stacks are captured and named.
- `with-private-tmp`, and every long-lived wrapper the sweep finds, cannot hang this way: 0 hangs in at least 100 churn runs, against the before rate.
- The dagrun wall-stalled reports are explained, or shown to be unrelated.
- The gate stays green.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2488 churn repro, in which one run hung this way after its test had passed. The system python (glibc malloc, no allocator thread) has not hung in this lane's loops since they moved to it.

Related:
- #2383: the private temp root that `with-private-tmp` sets up.
- #2472: a stopped gate leaves its dagrun scope running (a separate defect in the stop path).
