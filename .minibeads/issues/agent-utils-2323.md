---
title: 'vibe-talk-page-suite-negative-control-build-cache: the page suite builds each negative control bundle once instead of rebuilding the whole page through esbuild for every control'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- validation
- performance
created_at: 2026-11-26T19:30:00-05:00
updated_at: 2026-11-26T19:30:00-05:00
---

# Description

The app page suite (`tests/js/voice_page.test.mjs`) spends most of its CPU in the pinned esbuild service, not in the tests. About 316 negative controls (`brokenScript` / `brokenModule` in `tests/js/page/fixture.mjs`) each rebuild the page's prelude and entry through esbuild with their overlay patched in, two full builds per control. In a page slice killed at its 600 CPU-second guard on 2026-11-26, esbuild had used 390 of the 605 CPU seconds. `#2320` split the suite into six slices to keep each under its guard; this issue cuts the work itself.

# Design

Test code only, in the fixture. Options, in order of preference: build the unpatched module graph once per process and apply each overlay by re-bundling only from an in-memory module set (esbuild's incremental `context().rebuild()` or a plugin serving patched module text), or memoize the built bundle by its overlay set so a control repeated across tests is built once. Every control still runs against a real bundle containing its patch; no control is skipped or weakened, and a control whose patch no longer applies still fails exactly as today (`find` must occur `count` times).

# Acceptance Criteria

- The page suite's total CPU, measured with the six slices on a loaded host and solo, falls by at least half, with esbuild's share named in the measurement.
- Every negative control still builds a bundle containing its overlay; a test proves a control's patched text reaches the bundle it runs, and a control whose `find` no longer occurs still fails.
- No test removed or loosened; the suite's test count is unchanged.

# Notes

Measured by `cl-flaketriage` from the gate journals of 2026-11-26 (`process_snapshot` at the CPU kill: esbuild 390 CPU-s, node 213). Follow-up of `#2320`; its own lane and gate (coordinator, 2026-11-26).
