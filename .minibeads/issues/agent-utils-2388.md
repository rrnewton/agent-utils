---
title: 'gate-full-partial-delegated-live-scope: every green full gate ends PARTIAL because seven dagrun and cpuset-alloc cross cases skip inside the delegated validation cgroup, and no gate ever runs them'
status: open
priority: 1
issue_type: bug
assignee: cl-shardcap
labels:
- validation
- gate
- cross
- dagrun
created_at: 2026-11-27T10:47:00-05:00
updated_at: 2026-11-27T18:16:00-05:00
---

# Description

A full gate (`validate.py --all`) with every step green ends with verdict PARTIAL and exit 0, never PASS. The cause is seven cross-language cases that skip with kind `delegated-live-scope`:

- `cross.dagrun.cpuset-differential` skips five: `selftest:mutation-verdict`, `interop:py-then-rs`, `interop:rs-then-py`, `run:wrapped-help-passthrough`, `run:signal-status`.
- `cross.dagrun.differential` skips two: `pin-run:reserve-apply-release`, `pin-run:signal-status`.

Both steps PASS in dagrun. Each step log says `cross[...]: PARTIAL ... N check(s) were skipped and are UNVERIFIED (allowed skips by kind: delegated-live-scope N)`, and validate turns those coverage records into its `validate: PARTIAL - every selected node passed, but 7 cross check(s) were skipped and are UNVERIFIED (...)` verdict. This was seen on 1d968c39 (ticket `1795781715057792618-b0f7a7fd`) and 7211ced2 (ticket `1795782479839527747-989d9645`), which are the two full-gate PARTIAL runs whose logs are still kept.

## Why they skip

The skips are by design, and nothing changed recently.

- **The guard.** Both steps set `delegated_children: true` in `validation/cross.dag.yaml`, so dagrun gives them `DAGRUN_DELEGATED_CGROUP`. `cross/differential.py` (`_inside_delegated_harness()`) then skips every case that would start a live transient systemd scope.
- **Which cases.** The skipped cases are `cpuset-alloc run`, `selftest` and `dagrun pin-run`, which apply a HARD pin through `systemd-run --user --scope`.
- **Why that is unsafe here.** That would move the wrapped command into a sibling scope outside the step's cgroup. It would also leave the gate's CPU containment and its kill boundary.
- **What the code and YAML comments say.** The cases run only when someone runs `make cross` by hand at top level, and "no automated lane does" (GitHub issue 165). The durable fix is #28 delegated-cpuset, open since 2026-09-25.
- **Why a skip demotes the gate.** Two commits from 2026-09-28 made a skip its own result class, never a pass, and made validate report PARTIAL past dagrun's PASS: c264b965 and dcac8b86.

## Since when

Always, as far as the ledger reaches. The gate ledger starts on 2026-11-04.

- **First full-size gate.** f0e9abbe on 2026-11-07, 241 steps, was already PARTIAL.
- **No full gate has passed.**
  - 37 gate rows have at least 240 steps and `failed_steps` empty.
  - 36 of them are PARTIAL with exit 0, and one is PARTIAL with exit 3 (0bc61b7d, agentctl shards NO_RESULT).
  - None is PASS. The largest passing gate in the ledger has 238 steps.
- **Path-selected gates pass.** Gates from `validate.py --base github/main` (about 225 to 238 steps) pass because they select neither node unless the change touches dagrun or cpuset-alloc. Of the 20 kept gate log directories, only the five full gates ran the two nodes.
- **Not caused by recent containment changes.** The per-gate CPU containment (#1254) and the delegated-caller recipe (#2361) did not cause this. The delegation that triggers the skip is the validation DAG's own `delegated_children: true` on these two nodes.
- **Not checked.** The logs of PARTIAL rows older than the kept 20 directories are pruned. So their reason is assumed to be the same, not checked.

## What else stands in the way

These cases would not pass owner rule 346 as they are, even outside the delegated scope.

- **The interop pair races the clock.** The first command holds its reservation with `sleep 2`, and the second must reserve while the first is still alive. On a loaded host a slow second start lets the first release first. The two cores are then not guaranteed disjoint, and the check reports a false red.
- **The interop wait gives up silently.** The harness waits at most 10 s for the first reservation to appear, then runs the second anyway.
- **Three cpuset-alloc cases use the shared host ledger.**
  - The cases: `run:wrapped-help-passthrough`, `run:signal-status` and `selftest:mutation-verdict`.
  - The harness scrubs `DAGRUN_CORE_LEDGER` and these cases don't set it back, so the engines fall back to the host-wide reservation ledger (`$XDG_RUNTIME_DIR/dagrun/core-reservations.json`).
  - They therefore make brief real reservations in the ledger that other work on the host also uses.
  - The interop and pin-run cases already use scratch ledgers.

# Design

Measure the seven cases in every full gate instead of relabelling the verdict.

**Recommended near-term fix: a separate, non-delegated live-scope step.**

- **New selector.** `cross/differential.py` gets `--only-kind delegated-live-scope`. It runs only the cases that today skip with that kind, for `--tool dagrun` and `--tool cpuset-alloc`, and nothing else.
- **New step.** `validation/cross.dag.yaml` gets `cross.dagrun.live-scope`.
  - It has no `delegated_children`, so `DAGRUN_DELEGATED_CGROUP` is not set and the cases run.
  - Labels `validation-cross` and `component-dagrun`, so it is selected exactly when the two existing nodes are.
  - The existing nodes keep `--allow-skip delegated-live-scope` and keep listing the seven as skipped.
- **Coverage merge in validate.**
  - A label skipped as `delegated-live-scope` in one tool's record and passed in the same tool's live-scope record, in the same run, counts as measured.
  - A missing or skipped live-scope record still demotes to PARTIAL and names the labels, so coverage is never claimed without a run.
- **Hot-host fixes, part of the same change.**
  - **Interop hold.** The first command waits on a condition instead of `sleep 2`: it reads its stdin until EOF. The harness closes that pipe only after the second command has reserved and exited. If the harness dies, the pipe closes and the holder exits, so an aborted gate leaves nothing waiting.
  - **Interop wait.** The 10 s wait for the first reservation becomes a generous bound that fails with a named reason instead of running the second anyway.
  - **Scratch ledgers.** The three cpuset-alloc cases that use the host ledger get their own scratch `DAGRUN_CORE_LEDGER`, so no live case touches the host ledger.
- **What escapes containment, and for how long.** The live cases' transient scopes still leave the step's cgroup. Every wrapped command is short and ends on its own: `true`, `printf`, a self-SIGTERM, `selftest --sample-s 0`, and the stdin-held interop holder, which ends with its harness. The step comment says so, and why it is acceptable here.

**Durable fix: #28 delegated-cpuset.** It removes that escape.

- **Feasible here.** The user manager on this host delegates the `cpuset` controller. Its `cgroup.subtree_control` lists `cpuset cpu io memory pids`. dagrun's delegated subtree enables only `+cpu +memory +pids` today.
- **The change.**
  - dagrun enables `+cpuset` for steps that opt in.
  - Under a valid `DAGRUN_DELEGATED_CGROUP`, `cpuset-alloc run`, `selftest` and `dagrun pin-run` create a private child cgroup, write `cpuset.cpus`, run the command there and clean it up.
- **Result.** The seven cases then run inside the existing two nodes, with full containment, and the live-scope step can be retired.
- **Where cpuset is not delegated,** the skip stays and PARTIAL stays honest.

**Not recommended alone:** making `delegated-live-scope` skips stop demoting the verdict. That would call seven unmeasured checks green. It is acceptable only as a recorded interim waiver while the step above lands.

# Acceptance Criteria

- A full gate on a tree with every step green ends `validate: OK` and ledger verdict `pass`. Its journal shows `cross.dagrun.live-scope` passed, and the seven labels appear as checks that ran, not as skips.
- The two delegated nodes still list the seven as `delegated-live-scope` skips when run alone. validate counts them measured only when the live-scope record for the same tool and run passed them. A test pins both directions: merged gives OK; missing, skipped or failed live-scope gives PARTIAL naming the labels.
- No live case writes the host-wide reservation ledger. A test runs the step with a sentinel host ledger and checks that it is unchanged.
- The interop cases have no wall-clock hold. They pass 20 times in a row on a host with load1 above 237, and an aborted step leaves no holder process.
- Path-selected gates that do not touch dagrun or cpuset-alloc still skip all three nodes.

# Notes

- Related:
  - #28 delegated-cpuset is the durable fix: a delegated-root HARD-pin backend.
  - GitHub issue 165 records that no automated lane runs `make cross` at top level.
  - #1249 cross-differentials-robust brought the other cross differentials back without wall-clock verdicts.
  - #2361 records the delegated-caller cgroup routes.
- Owner rule 346: every gating check passes on a hot host and none depends on wall-clock time.
- Until this lands, the coordinator's standing waiver 1046 treats a gate row that is PARTIAL only for these seven skips as green for a held conditional landing. The waiver excludes changes to dagrun, cpuset-alloc or their cross harnesses.
- Found by cl-shardcap from the gate ledger, the kept gate logs and `validation/cross.dag.yaml`, on 2026-11-27.

## Landed; two criteria still owed

Landed as 3372b6f6 on 2026-11-27. The landing replayed 668e988f, gated on 0d40c24f. This issue stays open until the two criteria under "Still owed" are shown.

What the change does:

- `cross/differential.py --only-kind delegated-live-scope` runs only the live-scope cases, for `--tool dagrun` or `--tool cpuset-alloc`.
- The new node `cross.dagrun.live-scope` runs both tools outside the delegated validation cgroup, with the same labels as the two delegated nodes.
- validate counts a `delegated-live-scope` skip as measured only when a passing live-scope record of the same tool, from the same run, lists that check as passed.

Evidence per acceptance criterion, as of the landing:

- **Gate result.** In full gate `1795799002114893727-6977b0d9` on 668e988f, `cross.dagrun.live-scope` passed: "cross[dagrun]: OK - 2 live-scope checks agree", "cross[cpuset-alloc]: OK - 5 live-scope checks agree".
  - That gate was red on six steps this change cannot reach. All six were covered by passing reruns or step waivers (exclusive reruns `1795818247349059183-4dfc30a8` and `1795818236045163001-470a97e4`).
  - So the `validate: OK` summary and the `pass` verdict are not yet shown. validate returns before its summary on a red DAG (#2407).
  - The first all-green full gate on a tree with this change is the check. `test_validate_says_ok_when_the_live_scope_node_measured_every_delegated_skip` pins the outcome.
- **Both directions.** `test_validate_counts_a_delegated_skip_as_measured_only_by_a_passing_live_record` covers merged against missing, skipped or failed live-scope records; the failing cases end PARTIAL and name the labels. `test_a_live_scope_run_inside_a_delegated_cgroup_still_skips_and_is_incomplete` covers the delegated nodes run alone.
- **No host ledger writes.** `test_a_live_scope_cpuset_run_measures_exactly_the_five_live_cases` runs the cases with a sentinel host ledger. Every engine call, the holder included, used a scratch `DAGRUN_CORE_LEDGER`, and the sentinel is unchanged afterwards.
- **No wall-clock hold.** The interop holder now keeps its core until its stdin closes, replacing `sleep 2`. `test_a_live_scope_interop_holder_that_never_reserves_is_reported_not_raced` checks that a holder which never reserves gets a named failure, not a race.
  - 20 consecutive live runs of both tools all passed, but at load1 146-160. That is below the 237 that counts as a hot host, so the hot-host repetition is not yet shown.
- **Path selection.** `test_the_live_scope_node_runs_both_tools_outside_the_delegated_cgroup` shows the node is selected with the two nodes it measures, and only with them. The graph contract treats it as their complement, not as a second copy.

Still owed:

- A full gate in which every step is green ends `validate: OK` with ledger verdict `pass`, and its journal shows the seven labels as checks that ran. The first all-green full gate on a tree containing 3372b6f6 is the check.
- The interop live cases pass 20 times in a row at load1 of at least 237 throughout, with load1 recorded per run.

Waiver 1046 (a gate PARTIAL only for these seven skips counts as green) was put to the liaison for retirement on the evidence above.
