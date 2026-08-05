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
7. Treat adversarial review as the default for every PR. Only a documented process exemption may
   skip it; missing review evidence is not approval.
8. For a policy-changing PR, inspect the rationale date and compare it with the current policy before
   dispatch. A rationale that predates the policy it would change requires coordinator review.

The planner is advisory. For Hermit, execute an approved landing through the tracked landing executor,
which owns serialization and fresh-base checks. It must merge with `--rebase`, never `--admin`, then
fetch the destination and prove the landed commit is its ancestor. A GitHub API `MERGED` flag is not
landing evidence.

## Large-backlog and coalesced landing

Local validation is the main landing driver. GitHub CI is an independent supplement, not a 40-minute
inner loop and not a reason to postpone evidence the local producer can generate now.

### Produce a qualifying local record

On the measured Meta agent sandbox, bare `./validate.sh` exits 3 in about nine seconds because
BpfJailer denies cgroup creation. That is admission refusal, not a test result. Run the exact checkout
in a detached user scope instead:

```bash
systemd-run --user --unit=validate-<pr>-<sha> \
  --working-directory=<checkout> --collect \
  /bin/bash -c 'exec env PR_NUMBER=<pr> with-proxy ./validate.sh > <durable-log> 2>&1'
```

This is not an unboxed bypass: systemd grants the cgroup scope and the validation remains boxed. The
detached unit survives agent recycling. Use a unique unit and durable log, record both, and stop it
explicitly with `systemctl --user stop <unit>` (or the repository wrapper); killing the watching agent
does not stop the run.

Read duration before interpreting the log. On this lane, about 9 seconds means admission refusal,
about 137 seconds means a build/lint failure, and roughly 400–700 seconds means a real full run. These
ranges classify where to look; the receipt remains the authority.

`result=pass` alone is not a green. A qualifying Hermit receipt satisfies all of:

```text
result=pass AND profile=full AND checks=6 AND executed_tests>0 AND exact tested SHA
```

The current six-check count belongs to the current full profile and must move with that profile when
its declared gate count changes. A two-check `portable-strict-compat-only` row can also say `pass` and
often lands in the ledger first; it is not a full green. A feature/filter configuration can execute
zero tests and still print success; missing or zero execution is NO-RESULT. Select by exact SHA,
profile, coverage, and nonzero execution before ordering records by recency.

Treat **soft green** only as a scheduling signal: no known product failure and enough evidence to
admit a PR to a chosen batch. It never authorizes landing by itself. **Hard green** is the qualifying
exact-head full receipt above (or the repository's authoritative equivalent), with required review
resolved. State which one is being cited; never write only “green”.

### Choose serial or coalesced landing

A receipt is SHA-keyed. Rebasing changes the SHA and destroys its authority, so N serial landings pay
for N rebases and N exact-head validations. For a large conflict-free backlog, a coordinator-approved
staging branch can reduce that to one integration update and one full validation:

1. Fetch fresh and create the staging branch from **current main**, never from a stale green anchor;
   the latter silently reverts everything landed after the anchor.
2. Merge every ready, conflict-free PR into staging. Skip conflicts instead of resolving them inside
   the batch; conflict resolution changes the reviewed change and returns that PR to individual work.
3. Validate the combined exact staging head once. If main moves, update staging before validation;
   never claim the old receipt for the new head.
4. Land the staging change under the normal lock and protection rules.

Merging staging does not close the constituent PRs. GitHub recognizes commit lineage, not content
equivalence. After a fresh fetch, close only a constituent whose original head satisfies
`git merge-base --is-ancestor <pr-head> refs/remotes/origin/main` with rc=0. A non-ancestral head is
not proven landed; closing it can silently bury dropped work. This conservative constituent check is
distinct from proof of the staging/direct merge itself: that proof uses `mergeCommit.oid` plus
`git merge-base --is-ancestor <merge-oid> refs/remotes/origin/main` after fetching the named branch.
The API `MERGED` flag, successful `gh pr merge` exit, mergeability query, and clean dry-run are not
landing evidence.

### Serialize evidence and verification

Never batch local validates concurrently. Contention has triggered `detcore_misc` livelock and written
false reds; because recorded reds are not automatically retried, an operator-created false red can
permanently condemn a healthy PR. Hold the validation lock and derive any future safe width from a
solo run's measured CPU and memory footprint, not host core count.

Serialize merge operations too. Concurrent stale-base `--rebase --admin` merges orphaned already
merged work by replaying incompatible views of main. `--admin` is forbidden regardless; hold one
landing lock, fetch fresh before each merge, and ancestry-verify afterward.

Speculative landing is limited to CI-irrelevant diffs and owner-requested tooling fixes. Its contract
is binding: arm local validation and GitHub CI on the landed commit immediately, in parallel, and act
as soon as either reports. A speculative land without armed verification is a process violation, not
a shortcut.

During implementation, reproduce and iterate on the single failing test locally. Never put full CI in
the edit/test inner loop. Use full local validation as post-hoc confirmation, and run local validation
and GitHub CI in parallel rather than serially.

The planner must expose the conflict-safe groups and evidence class needed for this choice. The
coordinator owns the decision to coalesce because integration changes the meaning of per-PR evidence
and closure; the planner never creates or merges the staging branch.

## Commands

- `pr-landing-planner plan --repo OWNER/NAME --base BASE --git-dir /path/to/clone --net-wrapper with-proxy --landing-context context.json --format json`
- `pr-landing-planner plan --fixture demo.yaml --landing-context context.json`
- `pr-landing-planner graph` / `status` for the narrower views.
- `pr-landing-planner quickstart` and `--userguide` for the complete CLI reference.

The JSON schema exposes `assigned_agent`, `validation_evidence`, and `policy_class` on each node,
plus top-level `mechanism_overlap_edges` and `unclassified_mechanism_candidates`. Never infer those
fields from prose after the plan has been emitted.
