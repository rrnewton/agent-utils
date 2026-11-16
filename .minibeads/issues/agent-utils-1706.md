---
title: 'wrkslots-doctor-speed: doctor slowed about 9x; give periodic gates a fast bounded mode'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T12:22:08.000000000+00:00
updated_at: 2026-11-16T12:22:08.000000000+00:00
---

# Description

Reported by a consuming team's lead on 2026-11-16. An uncapped `wrkslots
doctor` on a busy host now takes 90.6 s (exit 0, 953 findings, of which
0-15 s is lock wait), against a 10.05 s baseline a month earlier when it
reported about 290 findings. That team's periodic health gate allows 45 s,
so it has produced no real result for five days (111 consecutive
"unavailable" ticks: 101 over 45 s, 8 lock-busy).

Unknown so far: what dominates the time. Candidates: finding count growth,
per-slot or per-worktree git calls, registry and journal size, archive or
event-log scans, /proc scans, and how long the lock is held.

# Design

- Measure first: profile one doctor run on a large registry fixture and on
  a real host's read-only copy, and name the top costs (per phase and per
  slot), with counts of subprocesses spawned.
- Fix the dominant costs: batch git calls per repository, cache per-run
  results that are computed once per slot today more than once, avoid
  rescanning event logs, and hold locks only around the state snapshot,
  doing the slow checks on the snapshot without the lock.
- Add a fast bounded mode for periodic gates (for example `doctor --quick`
  or `--summary`): only the checks that read the registry snapshot and
  cheap metadata, a documented time bound on a busy host, a machine-readable
  summary (counts by finding class, interrupted mutations, the slowest
  check), and a clear statement of what it skips. The full doctor stays
  the complete check.
- Report doctor's own timing per phase in --json so slowdowns show up
  before a gate times out.

# Acceptance Criteria

- A profile of the large fixture is recorded in the commit message or
  Notes, before and after.
- Full doctor on the fixture is at least several times faster, or the
  Notes explain why it can't be and what the fast mode covers instead.
- The fast mode finishes within its documented bound on the fixture under
  load (condition-based test, no wall-clock assertion), and its JSON
  carries per-class counts and per-phase timing.
- The lock is held only around the snapshot; a test shows another wrkslots
  command proceeds while doctor's slow checks run.
- Python and Rust editions agree where both implement doctor.

# Notes

Related: #1700 (status and doctor report interrupted mutations), #1701
(recover-absent holds every lock while thrashing), #1276 (create and adopt
validate the whole registry under the exclusive lock).

## Profile (before, 2026-11-16)

cProfile of `wrkslots doctor --format json` on this host's real
registry (20 active slots, 124 findings): 8.7 s bare (3.5 user +
4.6 sys), 12.4 s under profiler. 452 subprocess.run calls (392
through `_GitVcs._run`), 10.8 s in subprocess wait. Top phases on
the same registry with timings enabled: owner_liveness 4.16 s,
worktree_config 4.08 s, shared_submodule 2.61 s, identity_drift
0.80 s, storage_census 0.55 s, snapshot 0.02 s; 392 git
subprocesses. Per-slot liveness probes (0.178 s each) and the two
submodule walks dominate; the lock is not held during them
(phased read). `doctor --quick` on the same registry: 0.65 s,
0 subprocesses.

## Resolution (part 1: timings + quick mode)

Full doctor --json now carries `timings` (per phase plus total),
`subprocesses`, and `counts_by_kind`. `doctor --quick` is the
snapshot-only gate mode: holds/releases/dead owners from the
snapshot, interrupted journals, per-class counts, `slowest_check`,
and a `skipped` list naming every omitted check; it spawns no
subprocess. Rust doctor-text implements the same `--quick` and
envelope fields. The optimisation commit (liveness batching, one
shared submodule census) follows separately with before/after
timings.
