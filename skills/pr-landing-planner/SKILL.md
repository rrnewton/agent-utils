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
3. Treat a raw `locally-validated` label as an observed cache hint, never as landing evidence. Only
   authoritative CI or a caller-supplied exact-head `clean-validate-record` can authorize landing
   without waiting for or refiring a stale merge gate.
4. Escalate `gate-policy` changes even when validation evidence is green. Validation proves the code
   passed; it does not approve a change to landing policy. Routine `ci-hygiene` remains autonomous.
5. Surface PRs that change the same MECHANISM as a `mechanism_overlap_edge`, even in DIFFERENT files
   under DIFFERENT SPELLINGS (the `cancel-in-progress` #1567-vs-#1575 near-miss git merges cleanly).
   A three-stage pipeline gets this right: DERIVE candidates mechanically from the diff and
   `mechanism:<slug>` labels (no agent); CLASSIFY each into a stable `Mechanism` enum by normalising
   spellings; CLUSTER on the ENUM VALUE, so `concurrency.cancel-in-progress` and `CANCEL_IN_PROGRESS`
   land in ONE bucket. CLASSIFY may return UNCLASSIFIED — a valid output surfaced as
   `unclassified_mechanism_candidates`, the signal the enum needs a new member (extend the
   recognition table in `pr_landing_planner/mechanism.py` offline). An edge requests coordinator
   review; it does not claim the two PRs have opposing intent.
6. Use `assigned_agent`, the ordered decisions, and the conflict-safe groups to dispatch the batch.
7. Treat adversarial review as the default for every PR. Supply exact-head review evidence; a bare
   `passed-review-*` label is not evidence. The execution doctrine defines final-head review binding.
8. For a policy-changing PR, inspect the rationale date and compare it with the current policy before
   dispatch. A rationale that predates the policy it would change requires coordinator review.

The planner is advisory. It stops after producing conflict-safe groups and evidence classes. Use
[PR landing operations](../pr-landing-operations/SKILL.md) to decide and execute rebases, validation,
coalescing, review refresh, merges, and ancestry proof. The consuming workspace's
[`AGENTS.md`](https://github.com/rrnewton/dev-hermit/blob/main/AGENTS.md) remains the authorization,
review, and repository-policy authority; neither skill may weaken it.

## Commands

- `pr-landing-planner plan --repo OWNER/NAME --base BASE --git-dir /path/to/clone --net-wrapper with-proxy --landing-context context.json --format json`
- `pr-landing-planner plan --fixture demo.yaml --landing-context context.json`
- `pr-landing-planner graph` / `status` for the narrower views.
- `pr-landing-planner quickstart` and `--userguide` for the complete CLI reference.

The JSON schema exposes `assigned_agent`, `validation_evidence`, and `policy_class` on each node,
plus top-level `mechanism_overlap_edges` and `unclassified_mechanism_candidates`. Never infer those
fields from prose after the plan has been emitted.
