---
title: 'dagrun-capture-races: keep a perf acknowledgement that lands as the step finishes, and check wprof''s exit at the window boundary'
status: closed
priority: 2
issue_type: bug
labels:
- dagrun
created_at: 2026-10-14T14:48:59.060559836+00:00
updated_at: 2026-10-14T16:06:56.446670529+00:00
closed_at: 2026-10-14T16:06:30.073799752+00:00
---

# Description

[opus 5.5] Found by the 2026-10-14 branch triage (ai_docs/transient/2026-10-14-branch-triage/REPORT.md). Unlanded work from closed PR #119 dagrun-results-cgroup-integrity, kept on archived/ci/dagrun-results-after-strict-reader (also archived/ci/dagrun-results-cgroup-integrity and archived/rescue/20260911-pr119-dagrun-results-cgroup-integrity-7d047119). (1) Both editions: send_perf_command (rs/dagrun/src/profile_capture.rs, near line 1695) and ProfilerControl._ack (py/dagrun/profile_capture.py, near line 1120) check finished and the deadline before reading, so an acknowledgement perf has already written is lost when the step ends at that moment, and a trial with complete perf data is recorded as a failed capture. Commit 7d047119 (the same patch as 2c44e156) fixes the Rust side: one nonblocking read on every pass before honouring finished or the deadline, generic over Read/Write, with barrier-ordered, deadline, EOF, malformed-ack and read-error tests. Python needs the same change. (2) Rust only: start_wprof_stop_controller sends SIGINT at the window deadline without re-checking exited(), so a profiler that exited in that gap is misreported; Python already re-checks in _WprofStopController._run. Commit cc844e44 adds stop_wprof_at_window_boundary and two pidfd tests. Not included: 565e8325's fail-closed cgroup teardown, which depends on an API main never adopted; whether cgroup.kill should wait for 'populated 0' is a separate owner decision.

# Acceptance Criteria

Both editions read a pending perf acknowledgement before honouring step completion or the deadline, with a regression test that orders the acknowledgement before completion; the Rust wprof stop controller re-checks exit at the window boundary, with tests; the dagrun differential still agrees; make validate passes.

# Notes

[opus 5.5] Closed: both editions now read a perf acknowledgement that lands as the step finishes (the finish flag and deadline are sampled before each read, with interleaving regression tests in each), and the Rust wprof stop controller now checks the workload and then the profiler's exit at the window boundary in the Python edition's order, with helper and controller tests. Deliberately not done: 565e8325's cgroup teardown, and the editions' differing wprof error wording ("profiling window" in Rust, "selected window" in Python). Unverified: make validate is not green, only because of two failures already present on base 5860c83c in wrkslots files this work does not touch (strict mypy on its tests, and the lifecycle partition count 1958 != 1947). Every check that sees this change passed, including both dagrun differentials.
