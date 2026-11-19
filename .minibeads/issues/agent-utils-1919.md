---
title: 'vibe-talk-esbuild-gomaxprocs: cap the esbuild service GOMAXPROCS so page slices stop burning budget'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- test-infrastructure
created_at: 2026-11-19T17:12:44-05:00
updated_at: 2026-11-19T17:12:44-05:00
---

# Description

[watermelon-full-preview] Finding (coordb, 2026-11-19 5:1x PM ET):
the vibe-talk page test suites (dag steps `vibe-talk.test.page-0..3`)
are being killed by their 600 s CPU budget mid-suite with no failing
test, in many gates today. Step attribution shows the esbuild service
process (`vibe-talk/contract/node_modules/esbuild ... --service`)
burning about 390 s of CPU beside node. Live sample: one esbuild
`--service` process was 5 s old with 15 s of CPU and 197 threads,
parked in futex waits. esbuild is a Go binary: with no `GOMAXPROCS`
set, Go sizes its scheduler to all CPUs on a many-core host (316
hardware threads here), and idle Go worker threads spin looking for
work, which burns CPU-seconds for little gain. Owner rule 346: tests
must be robust on a hot host.

# Design

Measure first: run one page slice (the `node --test` invocation the
dag step runs) under `nice -n 10`, recording total CPU seconds for
node and esbuild separately plus wall time, for the uncapped baseline
and for `GOMAXPROCS=2`, `4` and `8` in esbuild's environment (the
esbuild JS API spawns the service as a child of the parent node
process and inherits its environment, so setting it in the test
runner's or the dag step's environment caps it). Then cap
`GOMAXPROCS` everywhere the tests and the page build start esbuild,
at the value the measurements choose. Also check whether one esbuild
service is started per test file and could be shared or reused;
report it, and fix it only if simple. Use a dag.d fragment or
Makefile env only where needed.

# Acceptance Criteria

- Before and after CPU seconds per slice (node and esbuild
  separately) and wall time are recorded in the resolution.
- The chosen cap is applied everywhere the tests and page build
  start esbuild, with no change to page output.
- Page build output is byte-identical with and without the cap.
- Whether an esbuild service is started per test file, and whether
  it could be shared, is reported in the resolution.

# Notes

Lane mu-esbuildcap/1919. Validator identity: hermit-bot3. Nested
worktree `.muse/worktrees/esbuildcap` on branch `mu-esbuildcap/1919`.
New vibe-talk checks go in `tests/dag.d/<next-free-NNN>-<job>.yaml`;
numbers are never reused (next free at filing: 168).
