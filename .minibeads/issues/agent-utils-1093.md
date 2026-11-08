---
title: 'case-timings-sharder-duplicate: a pytest-sharder timing record with a duplicate primary case fails validate after every step passed'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-08T22:05:00.000000000+00:00
updated_at: 2026-11-08T23:10:00.000000000+00:00
closed_at: 2026-11-08T23:10:00.000000000+00:00
---

# Description

A full gate can pass every selected DAG step and still exit non-zero on validate's per-case timing inventory check:

> validate: case timings: pytest-repository.hygiene.pytest-sharder-<pid>-1.json: duplicate primary case test_fails.py::test_fails for pytest gate repository.hygiene.pytest-sharder
> validate: FAILED - per-case timing records do not match their collected inventories

Seen on 2026-11-08 21:45Z in the #624 move 4 gate on b7cf04b3 (150/150 steps passed). A rerun of `repository.hygiene.pytest-sharder` and `python.repository-infrastructure.test` passed. That change touched only vibe-talk page code. The gate ledger has ten gates since 2026-11-05 with a non-zero exit and an empty `failed_steps` (8a6c0f3d, 61e717be, db9326b9, 930089c6, 75638f31, 7d74e03f, 93f440cd, 21e253de, 0e2ef7c5, b7cf04b3). Not all of them are necessarily this cause, but each one costs a coordinator a manual judgement before landing.

`test_fails.py::test_fails` looks like a fixture of the pytest-sharder self-test, a nested pytest run. Its cases seem to reach the gate's timing directory (`AGENT_UTILS_CASE_TIMING_DIR`) as primary records, so the nested-run marker (`AGENT_UTILS_CASE_TIMING_NESTED`, set by scripts/run_pytest_shard.py) is apparently not applied on this path, or is applied only some of the time.

# Design

- Find how the sharder self-test's nested pytest gets the timing directory without the nested marker. Isolate it: nested runs either get no timing directory or are marked nested.
- Classify the ten ledger runs above: which share this cause, and which have another post-step failure.
- If validate fails after all steps passed, say so plainly in the gate verdict (for example a distinct exit code or ledger field) so a coordinator can tell bookkeeping from a red step.

# Acceptance Criteria

- A test runs the sharder self-test under a gate-like timing directory and validate's inventory check passes.
- The ledger classification of the ten runs is recorded under Notes.
- A validate failure after all steps passed is distinguishable in the ledger.

# Notes

Filed by the devbig030 coordinator, 2026-11-08. Move 4 landed as 81884c6a on the evidence that all 150 steps passed plus the two-step rerun.

## Resolution

Cause: #1064 test-temp-and-mount-leaks (7a9aea79) added `_self_test_keep_on_failure` to `scripts/run_pytest_shard.py --self-test`. It runs the wrapper twice on a fixture `test_fails.py` with the gate's environment, including `AGENT_UTILS_CASE_TIMING_DIR`. The self-test runs outside pytest, so the nested marker is never set, and both fixture runs wrote primary timing records for `test_fails.py::test_fails` into the gate's directory. Every gate whose tree contained 7a9aea79 then failed validate's inventory check (b7cf04b3, cafb1cd1 and 0514bccf, all with every DAG step passing). The gate on e016c287, based before 7a9aea79, passed.

Fix: the self-test's fixture runs (keep-on-failure and SIGTERM cleanup) now get `_fixture_environment()`: a private TMPDIR and no timing directory. Reproduced by running `--self-test` with `AGENT_UTILS_CASE_TIMING_DIR` set: main's version left two primary records naming `test_fails`, and the fixed version leaves none.

Not done: the earlier non-zero-exit gates with no failed step (2026-11-05 to the morning of 2026-11-08) predate 7a9aea79 and have some other cause. Classifying them, and a ledger field that marks a validate failure after all steps passed, were not part of this fix; file them separately if they recur.

