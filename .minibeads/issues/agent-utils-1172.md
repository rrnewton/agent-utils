---
title: "audit-lock-hold: an hourly wrkslots audit holds the registry lock for minutes at 100+ slots"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T23:35:00.000000000+00:00
updated_at: 2026-11-09T23:35:00.000000000+00:00
---

# Description

A consumer runs `wrkslots --wait-lock 300 audit --format json --gate` hourly from a user timer. Measured
2026-11-09 by that consumer's profiling lane: at 105 slots one audit took 244.3 s wall and 116 CPU s, and was still
running when the sample ended. It contends for the registry lock: a slot `adopt` under `--wait-lock 600` waited
minutes behind it. #1108 remove-lock-hold-starves-validate and #1142 image-remove-fence-first fixed the same shape
for remove (slow checks under the registry locks); audit still has it.

# Design

1. Measure first: where audit spends its time (process census, git status, salvage and unpushed checks, image or
   mount checks, journal parsing), and how long it holds the registry lock, per slot and in total. Reuse the #1108
   lock-hold measurement approach (wrap `_locked` and the expensive helpers, print a timeline).
2. Hold the lock only for a snapshot: read the registry rows and journal state under the lock, release it, do the
   expensive per-slot work (census, git status, salvage checks) outside, then re-validate briefly under the lock only
   what the verdict needs (row generation and owner unchanged, no new journal). A slot that changed meanwhile is
   reported as changed-during-audit, never as a false finding.
3. Make it cheaper per slot or incremental: skip expensive checks for slots unchanged since the last audit (by
   generation, row mtime, and a cheap checkout fingerprint), and keep the census shared across slots (one /proc pass,
   not one per slot).
4. Say whether the Rust readers (#1122 parity; their soak is paused) would already help, and how much.

# Acceptance Criteria

- At 100+ slots, the hourly audit holds the registry lock for seconds, not minutes; total wall time drops
  measurably. Before and after numbers (wall, CPU, lock hold) are recorded in Notes.
- `--gate` verdicts are unchanged for the same state (a differential test against the current audit on a fixture
  with findings).
- A concurrent `adopt` or `create` is no longer starved: test or measure the wait behind a running audit.
- Rust parity filed or done if the Rust audit shares the shape.

# Notes

Reported by the lead coordinator from a consumer's profiling lane (2026-11-09). Same family as #1108 and #1142.
