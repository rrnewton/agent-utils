---
title: 'wrkslots-census-never-closes-on-busy-host: proven-dead generations keep the same-UID census open'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- census
created_at: 2026-11-19T01:30:00+00:00
updated_at: 2026-11-19T01:30:00+00:00
---

# Description

[watermelon-full-preview] Consumer evidence, 2026-11-18/19: `wrkslots recover` against a validation-batch seal was refused three times (140-162 s each) with "same-UID process census did not close: each of 8 process inventories found generations that may still use the target". The seal's launcher and operator processes are both gone and the sealed slot's directory no longer exists, but the host always has same-UID processes starting (~60 lanes plus timers), so the census never closes. Every remove/adopt/finish touching that seal stays blocked, and the consumer's removal queue is stalled.

Cause: a generation that vanished or changed before its scan completed counted as "may still use the target" exactly like a live one, and closure demanded two consecutive rounds with zero such generations. Unrelated churn alone defeats it.

# Design

Death is proof: a generation proven gone (PID unassigned, PID reused by another generation, or terminal with its leader's threads released) drops out of the relevant set. It is followed, not counted: the census takes another inventory to meet whatever it forked before dying, and an endless handoff chain has its own bounded followed-rounds refusal. Closure requires two consecutive rounds with no LIVE not-proven generation; a live generation that changed identity stays relevant (fork-follow), and a positive match still refuses. The finish fence plus recheck before deletion guards the residual race, unchanged.

Acceptance fixture (from the consumer): a seal journal whose named processes are all dead and whose sealed directory is absent must close even under heavy unrelated churn (scripted-host test: unrelated generations born and dying between rounds), and a live holder that keeps forking must still refuse within the bound.

# Acceptance Criteria

- The churn fixture closes; the endless-handoff fixture still refuses within the bound.
- Existing census suites (late-fork, threads, binding-deadline, subject-progress, state-bounds, traversal-deadline) pass.
- Both editions where the census surface exists (the same-UID following census is Python-only today; Rust recover paths confirm no twin needs the change).

# Notes

Blocks the consumer's validate runs and removal queue; owner-priority gate.
