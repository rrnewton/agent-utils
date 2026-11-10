---
title: 'keyboard-browser-cpu-deflake: the keyboard browser check hits its CPU budget under gate load'
status: open
priority: 2
issue_type: bug
assignee: mu-foldlat
created_at: 2026-11-10T19:33:47+00:00
updated_at: 2026-11-10T19:33:47+00:00
---

# Description

[opus 5.5] `vibe-talk.check.keyboard-browser` fails with a CPU timeout under gate load and passes on a solo rerun. On 2026-11-10 it did so in three lanes' full gates on one host (each on a change that did not touch the keyboard code), and on 2026-11-09 in an owner UI --all gate. It is a long browser step: in a passing full gate it took about 100 s of wall time and 42 s of user CPU, so its CPU budget sits close to what the step needs when the host is contended.

This is the third load-shaped browser check after #1231 fold-latency-deflake and #1243 keyboard-bar-settle, and the mechanism may differ from both.

# Design

- Measure first: record the step's CPU time, wall time and host load (gate admission running count, loadavg, PSI) per run, solo and under a representative gate, and print them on failure.
- Find what the budget is protecting (a real per-key work bound, or only a step-level runaway guard) and fix accordingly: make a work bound load-aware the way #1231 did, or size a runaway guard from measured solo cost with clear headroom, or move the step to an exclusive tail if contention cannot be corrected for.
- A deliberate slowdown in the keyboard path (a mutation adding work per key) must still fail, solo and under load.

# Acceptance Criteria

- 10 of 10 under a representative gate load (daytime, bounded), and a solo pass.
- The slowdown mutation still fails.
- The failure message prints the measured CPU, wall and load evidence.

# Notes

Lead request 2026-11-10. Owner: mu-foldlat (after #1243).
