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
