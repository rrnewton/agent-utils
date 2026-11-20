---
title: 'vibe-talk-esbuild-gomaxprocs: cap the esbuild service GOMAXPROCS so page slices stop burning budget'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- test-infrastructure
created_at: 2026-11-19T17:12:44-05:00
updated_at: 2026-11-19T19:01:09-05:00
closed_at: 2026-11-19T19:01:09-05:00
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

Lane mu-esbuildcap/1919, on branch `mu-esbuildcap/1919`.
New vibe-talk checks go in `tests/dag.d/<next-free-NNN>-<job>.yaml`;
numbers are never reused (next free at filing: 168).

## Resolution

Landed 243be0a0 on github/main (fix da02d93a, replayed from gated
e27e1718; gate ledger verdict pass, 199 steps, 2026-11-19). The cap
lives in one place: `loadTool("esbuild")` in `vibe-talk/web/build.mjs`
sets `GOMAXPROCS=4` when the caller set none, before the JS API
lazily spawns its Go service from that node process, so every test
and page build that loads esbuild is capped; a caller's own
`GOMAXPROCS` is left alone. No dag.d or Makefile env was needed.

Measurements (page shard 0/4, 642 of 2565 tests, the dag page-0
invocation under `nice -n 10`, per-process CPU from /proc on
2026-11-19): uncapped, esbuild used 486 and 455 CPU-s against
node's 209 and 183 (wall 189 and 165 s, 244-249 esbuild threads);
`GOMAXPROCS=2` gave esbuild 121, node 174, wall 195 s; `4` gave
119, 170, 160 s; `8` gave 138, 179, 160 s. After the fix, with no
env set: esbuild 148 CPU-s, node 216, all 642 tests pass. A
standalone page build used 6.4 esbuild CPU-s simulated-uncapped
against 2.1 capped, so a full gate saves roughly 1,400 CPU-s:
about 1,360 from the four slices and about 40-60 from page builds
(the browser checks serve the page embedded in the cargo-built
binary and start no esbuild themselves).

Page output: `node web/build.mjs` output trees are byte-identical
uncapped against capped, and on the fixed sources identical
including `assets.json` between the defaulted cap and
`GOMAXPROCS=8`. Service sharing: the JS API keeps one long-lived
service per node process, so a slice already shares one service
(measured: exactly one per slice); each separate `node --test`
file process starts its own, and sharing across those processes
is not simple, so it is left as an optional follow-up. The new
`page_build` test reads the spawned service's own `GOMAXPROCS`
back from /proc and holds the caller's-override case; the suite
passes 27/27 and the gated tree passed the full gate.
