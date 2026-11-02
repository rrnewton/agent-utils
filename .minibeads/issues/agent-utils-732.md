---
title: 'local-merge-queue: a local merge queue that orders, batches, validates and lands ready changes'
status: in_progress
priority: 1
issue_type: feature
created_at: 2026-11-02T01:45:00+00:00
updated_at: 2026-11-02T01:45:00+00:00
---

# Description

[opus 5.5] The owner (2026-11-01): build a merge queue prototype that runs locally (no hosted service, no webhooks; the hosts take no inbound connections), so a consumer's coordinator can mark many changes ready and let the queue order and chunk them, raising landing throughput while keeping the base branch green. It extends and replaces the advisory landing planner as the first step of a drain.

Milestones, each landed on main and usable alone:

- M1 `plan`: from the changes submitted as ready, order them by priority, the exact conflict graph (`git merge-tree`, reusing the planner's graph code) and age, and chunk them into batches; human and `--json` output.
- M2 `run --dry-run` / `run`: build each batch's candidate (base plus the batch's changes in order), validate it, land it through a configurable land driver when green, bisect a red batch to the culprit and eject it with a report, and never land an unvalidated combination. At least two validations at once from the start: a 2-deep speculative pipeline (batch k+1 validated on top of batch k) and concurrent bisection halves, honouring host admission control.
- M3 throughput: a configurable window, and a PRs/hour measurement against serial landing.

Validation is a set of individual tests, never one boolean (owner, 2026-11-01): the validate interface takes a selection of tests (step IDs with optional case IDs; "all" is one selection) and returns per-test results; every submission and every landing records which tests it was validated with (focused or complete, plus the selection policy), and a focused landing leaves a tracked obligation to widen to the complete suite; bisection and retries run only the failed tests, never the whole suite.

No repair for now: a culprit is ejected with a report. Later: merge-conflict and repair agents, and multiple machines.
