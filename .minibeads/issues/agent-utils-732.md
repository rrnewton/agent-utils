---
title: 'local-merge-queue: a local merge queue that orders, batches, validates and lands ready changes'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-02T01:45:00+00:00
updated_at: 2026-11-02T03:40:00+00:00
closed_at: 2026-11-02T03:40:00+00:00
---

# Description

[opus 5.5] The owner (2026-11-01): build a merge queue prototype that runs locally (no hosted service, no webhooks; the hosts take no inbound connections), so a consumer's coordinator can mark many changes ready and let the queue order and chunk them, raising landing throughput while keeping the base branch green. It extends and replaces the advisory landing planner as the first step of a drain.

Milestones, each landed on main and usable alone:

- M1 `plan`: from the changes submitted as ready, order them by priority, the exact conflict graph (`git merge-tree`, reusing the planner's graph code) and age, and chunk them into batches; human and `--json` output.
- M2 `run --dry-run` / `run`: build each batch's candidate (base plus the batch's changes in order), validate it, land it through a configurable land driver when green, bisect a red batch to the culprit and eject it with a report, and never land an unvalidated combination. At least two validations at once from the start: a 2-deep speculative pipeline (batch k+1 validated on top of batch k) and concurrent bisection halves, honouring host admission control.
- M3 throughput: a configurable window, and a PRs/hour measurement against serial landing.

Validation is a set of individual tests, never one boolean (owner, 2026-11-01): the validate interface takes a selection of tests (step IDs with optional case IDs; "all" is one selection) and returns per-test results; every submission and every landing records which tests it was validated with (focused or complete, plus the selection policy), and a focused landing leaves a tracked obligation to widen to the complete suite; bisection and retries run only the failed tests, never the whole suite.

No repair for now: a culprit is ejected with a report. Later: merge-conflict and repair agents, and multiple machines.

# Resolution

[opus 5.5] Delivered on main:

- M1 `plan`: 3205bc24. Submissions bound to exact heads, exact `git merge-tree` conflict graph, conflict-free prioritised batches, held changes with unblocking actions, `--json`.
- M2 `run` / `widen` and M3 rolling window and `bench`: 918b8d76.
  - Validation as a set of tests (`command` and `dagrun` validators, per-test results, focused and complete claims).
  - A rolling speculative window (depth 2, AIMD to `--max-depth`), cancellation of discarded work.
  - A culprit search that runs only failed tests and needs the failure seen twice; flaky failures recorded, never blamed.
  - Delta revalidation of the remainder, widening obligations, `push` and per-change `command` land drivers, shadow and dry runs, opt-in union merges.
- Measured with `merge-queue bench` (24 changes, 2 s per validation): 3.72x the serial landing rate with no failures, 1.61x with 3 failing (depth 2, batch 4), 1.85x (depth 3, batch 3).
- Evidence on the planner's conflict graph, from 400 commits of this repository: `merge-tree` probes agreed with real combination on every pair and triple checked; file overlap flagged 96 pairs, all of which merged cleanly.

Unverified: the queue has not yet run against a consumer's real pull requests. A shadow kit (plan, dry run, shadow validation; lands nothing) was handed to the coordinator for that.

Follow-ups for new issues when wanted:
- merge-conflict and repair agents, and multiple machines;
- a probe cache;
- "applies on top of X" ordering edges;
- case-level focus for the `dagrun` validator.
