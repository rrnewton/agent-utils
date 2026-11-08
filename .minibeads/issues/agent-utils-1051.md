---
title: 'gate-merge-queue-cpu-budget: the merge-queue test step outgrew its 1200 CPU-s budget'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-08T08:37:43.000000000+00:00
updated_at: 2026-11-08T08:37:43.000000000+00:00
---

# Description

`rust.merge-queue.test` was killed CPU-TIMEOUT >1200s cpu in a
full `--all` gate (480 s wall, compile finished in 51 s, then
"no test events yet" until the kill; the same timeout hit an
unrelated lane's gate the same night with no merge-queue
changes). Measured solo in the exact step form (case_timings
wrapper, `--jobs 8`, `RUST_TEST_THREADS=4`, seven whole-crate
runs during #1045/#1049): tests alone take 738-873 CPU-s with
compilation cached (~75% sys: the integration suites spawn
queue, donor and git subprocess trees across 51 binaries), and
1031-1189 CPU-s in runs that also recompiled. The step therefore
sits at roughly two-thirds to effectively all of its 1200 CPU-s
budget before any gate contention; under a full gate (other
steps burning >1000 user-s concurrently on the shared box) sys
time inflates past the cap and the step dies with no verdict.
Worse, the kill loses all progress information: the step
reported 0 tests completed although most binaries had run,
because per-binary output is not preserved across the kill.

# Design

- Raise the step's CPU budget to 2400 CPU-s in the validation
  DAG: roughly 2x the worst measured solo consumption (1189
  CPU-s including recompilation), with the measurement recorded
  where the DAG carries resource reasoning. The step stays
  bounded — this is headroom for a process-heavy suite, not an
  exemption.
- Keep per-binary output across a CPU kill in the
  case_timings run-cargo wrapper (or the layer that drops it):
  when the step is killed, the log must show how far each test
  binary got (completed binaries' results, the binary in
  flight), so a timeout is attributable instead of reading as
  zero test events. Green-run output stays byte-compatible.
- A test for the wrapper change if one fits the existing
  case-timings test shape: a run killed mid-binary still
  reports the binaries that completed.

# Acceptance Criteria

- `rust.merge-queue.test` passes solo in step form and inside
  `gate.py --rerun` on a loaded host without approaching the
  new budget (measured CPU reported in the landing).
- A wrapper run killed mid-suite shows completed binaries'
  results in its output; the new wrapper test covers it.
- No other step's budget or lines change (a devbig030 cap
  change edits other lines of the same DAG file; keep this
  hunk to the one step).

# Notes

Measurements by mu-archive during #1045 merge-queue-donor-tool-identity
(see its Resolution for the solo table and the two unrelated
timing flakes the runs separated from this). Requested by the
graph owner after the CPU-cap finding was relayed; the gate's
shared phase otherwise passed 237 steps.
