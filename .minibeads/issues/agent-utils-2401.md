---
title: 'gate-row-passed-steps: a gate or rerun ledger row names the steps it ran green, so a landing tool can check that a rerun covered every failed step'
status: open
priority: 1
issue_type: feature
assignee: cl-flaketriage
labels:
- gate
- ledger
created_at: 2026-11-27T15:25:00-05:00
updated_at: 2026-11-27T15:25:00-05:00
---

# Description

`scripts/gate.py` writes one ledger row per run.
- A row names its `failed_steps`, `known_red_steps` and `incomplete_steps`.
- For the steps that passed it records only a count (`steps`), not their names.

A landing tool that accepts "the gate failed, then an exclusive rerun on the same tree passed" therefore cannot tell from the ledger which of the failed steps the rerun actually ran.
- The only record of the steps a rerun ran is its dagrun step journal under the row's `log_path`.
- gate.py keeps only its newest log directories, so that journal is often gone by landing time.

On 2026-11-27 a gate row failed 8 steps (`1795782448988793628-761f1ee4` on 1b540308). Its exclusive rerun (`1795808678088708906-3299e735`) ran only 2 of those steps and passed. The landing tool used here, which lives outside this repository, accepted the pair as evidence for all 8. A coordinator waiver happened to cover the other 6.

The landing tool's coverage check is being fixed where that tool lives. This issue is the agent-utils side: put the facts any landing tool needs into the row itself.

# Design

- Every ledger row (gate and rerun) records `passed_steps`: the sorted names of the steps that ran to a passing end in that run. They are taken from the same step results gate.py already reads to fill `failed_steps` and `incomplete_steps`.
- A row from an older gate.py has no `passed_steps`. Readers must treat that as "unknown", never as "none passed" or "all passed".
- `gate.py stats` and other readers ignore the new field unless they use it.
- No change to verdicts, exit codes or admission.

# Acceptance Criteria

- A gate run with passing, failing and NO_RESULT steps writes a row whose `passed_steps`, `failed_steps` and `incomplete_steps` are disjoint and together name every step that started.
- A `--rerun` row lists exactly the steps it ran green.
- A test reads a row without `passed_steps` and shows that existing readers (`gate.py stats`) still work.
- No wall-clock budget is added (owner rule 346). No new persisted wrkslots field.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-27), after the #2372 test-fix landing (906b54fc).

Related:
- #2355: unmeasured NO_RESULT steps; the same journal dependency for steps a rerun cleared.
- #2317: NO_RESULT visibility in rows.
- #2311: rows record the head and tree at the start and end.
