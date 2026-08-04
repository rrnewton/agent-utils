---
name: pr-landing-planner
description: Advisory PR landing planner that combines real merge conflicts, exact-head validation evidence, policy disposition, assigned agents, mechanism overlaps, freshness, and CI diagnosis. Use it before choosing or assigning a landing batch; it never mutates a PR.
---

# PR landing planner

Use the planner to produce the shared, machine-readable landing plan. Do not rebuild its conflict
map or readiness table by hand.

## Current contract

1. Collect open PRs and real pairwise conflicts with `git merge-tree` (the default detector).
2. Supply caller-owned facts with `--landing-context FILE`: exact-head clean validation evidence,
   `ci-hygiene` versus `gate-policy`, and the assigned agent. A `clean-validate-record` entry without
   an exact `head_sha`, or one whose SHA drifted, fails loudly.
3. Treat either the `locally-validated` label or an exact-head `clean-validate-record` as sufficient
   validation evidence. Those signals do not wait for, or refire, a stale merge gate.
4. Escalate `gate-policy` changes even when validation evidence is green. Validation proves the code
   passed; it does not approve a change to landing policy. Routine `ci-hygiene` remains autonomous.
5. Surface every shared `mechanism:<slug>` label as a `mechanism_overlap_edge`. This requests
   coordinator review; it does not claim the two PRs have opposing intent.
6. Use `assigned_agent`, the ordered decisions, and the conflict-safe groups to dispatch the batch.
7. Treat adversarial review as the default for every PR. Only a documented process exemption may
   skip it; missing review evidence is not approval.
8. For a policy-changing PR, inspect the rationale date and compare it with the current policy before
   dispatch. A rationale that predates the policy it would change requires coordinator review.

The planner is advisory. For Hermit, execute an approved landing through the tracked landing executor,
which owns serialization and fresh-base checks. It must merge with `--rebase`, never `--admin`, then
fetch the destination and prove the landed commit is its ancestor. A GitHub API `MERGED` flag is not
landing evidence.

## Commands

- `pr-landing-planner plan --repo OWNER/NAME --base BASE --git-dir /path/to/clone --net-wrapper with-proxy --landing-context context.json --format json`
- `pr-landing-planner plan --fixture demo.yaml --landing-context context.json`
- `pr-landing-planner graph` / `status` for the narrower views.
- `pr-landing-planner quickstart` and `--userguide` for the complete CLI reference.

The JSON schema exposes `assigned_agent`, `validation_evidence`, and `policy_class` on each node,
plus top-level `mechanism_overlap_edges`. Never infer those fields from prose after the plan has
been emitted.
