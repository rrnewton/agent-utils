---
title: 'gate-evidence-incomplete-steps-refused: the landing wrapper refuses gate evidence that leaves selected steps unmeasured (NO_RESULT) unless a recorded waiver names them for that commit'
status: open
priority: 1
issue_type: bug
assignee: cl-shardcap
labels:
- gate
- landing
- validation
created_at: 2026-11-27T00:38:00-05:00
updated_at: 2026-11-27T00:38:00-05:00
---

# Description

Since #2207, a run with a step that ends NO_RESULT exits 3. gate.py now lists those steps in the ledger row as `incomplete_steps`. NO_RESULT means the step was never measured, for example after an OOM kill at a learned memory cap.

The harness landing wrapper (`land-main`, kept in the operator's harness repository, not here) never reads `incomplete_steps`. It accepts three kinds of evidence for a commit's tree:

- the newest gate passed;
- the newest gate was partial with exit 0 and no failed steps;
- the newest gate failed, and a later exclusive rerun passed.

On 2026-11-27 a commit landed on the third path. The gate's one failed step passed in a rerun, but in that same gate `python.agentctl.shard-0` and `python.agentctl.shard-1` had ended NO_RESULT. The rerun ran only the failed step, so the agentctl Python suites (about 2900 tests) were never measured on the landed tree. This is the same blind spot that hid those shards for six and a half days (#2306), now at landing time.

# Design

Every evidence path applies the same rule:

- **Unmeasured steps:** take the `incomplete_steps` of the newest gate on the commit's tree, plus those of every passing exclusive rerun after it. Subtract the steps that some passing rerun after the gate ran to a passing end.
- **A rerun clears only the steps it ran.** A step that's incomplete in the gate and absent from the rerun stays unmeasured.
- **Where a rerun's steps come from:** a ledger row records only a count of passed steps. So the wrapper reads the rerun's dagrun step journal (`journal.jsonl` under the row's `log_path`, `step_end` events with `ok`). The last `step_end` for a step decides. A missing journal clears nothing; gate.py keeps only its newest 20 log directories, so a journal can be gone by landing time.
- **Refusal:** any unmeasured step refuses the landing (exit 3), naming each step and saying how to clear it: a rerun that runs it, or a waiver.
- **Waiver:** it reuses the recorded-waiver file that already governs `--no-gate-evidence` on a code change. That file is written by the coordinator, not by lanes. A step is waived when one line names the commit's first 8 hex digits and the step's exact name. Shorthand such as `shard-0/1` doesn't count, and `shard-1` doesn't cover `shard-10`.
- **Where it applies:** the rule covers the pass, partial and fail-plus-rerun paths. The wrapper's `--help` says so in one line.

Out of scope here (#2317): recording per-step NO_RESULT outcomes and passed step names in the ledger row, and the alarm for consecutive NO_RESULT gates.

# Acceptance Criteria

- A test script builds a scratch ledger, scratch waiver file, scratch step journals and a scratch repository with a local remote. It never reads the real ledger, waiver file or remote. It covers:
  - each evidence path, with and without a waiver;
  - the 2026-11-27 shape (a failed step rerun green, NO_RESULT steps left out of the rerun), which is refused and names the steps;
  - a rerun that ran the incomplete steps, which is accepted;
  - two reruns that together cover them, which is accepted;
  - a missing rerun journal, which is refused;
  - a step that ended NO_RESULT again in the rerun, which is refused;
  - a waiver for another commit, or one naming only some of the steps, which is refused and names the rest;
  - shorthand and prefix step names, which don't waive.
- Evidence without incomplete steps lands exactly as before.
- `land-main --help` documents the rule.

# Notes

- #2317 is the agent-utils side: it makes the row name each NO_RESULT step, raises the consecutive-gate alarm, and makes landing tools refuse an incomplete suite.
- #2306 is the six-and-a-half-day agentctl shard blind spot behind this.
- #2207 made NO_RESULT runs exit 3 and record `incomplete_steps`.
- The fix lives in the harness repository. Its commit is recorded here when it lands.
- Possible follow-up for #2317: once gate.py records each run's passed step names in the row, the wrapper can read them there instead of from log directories that get pruned.
