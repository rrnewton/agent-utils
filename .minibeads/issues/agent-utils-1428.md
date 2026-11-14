---
title: 'detached-runner-unknown-as-hit: UNKNOWN outcome credited as HIT'
status: closed
priority: 2
issue_type: bug
labels:
- parallel-experiment-runner
created_at: 2026-11-14T13:35:01-05:00
updated_at: 2026-11-14T13:35:01-05:00
---

# Description

[muse watermelon] The detached experiment runner in `py/parallel_experiment_runner/` could credit an UNKNOWN outcome as a HIT. A counter-complete detached resource report whose `resource_report_state` is `INCOMPLETE` ("required terminal Result unknown"), produced by a wrapping launcher (a resource-guarded launcher) that exits 193 when it cannot obtain the wrapped unit's terminal Result, with zero breach counters and a worker log matching the hit regex, was classified HIT.

The report parser ignored the state field entirely, the execute classifier only checked breach counters before delegating to the hit/miss classifier, and that classifier returned HIT on a log-regex match for any exit code, including 193, a missing exit code, or a signal death. Sites at filing (base 636bbbf3, line numbers since moved): `py/parallel_experiment_runner/detached.py` (report parsing), `py/parallel_experiment_runner/execute.py` (`_classify_outcome`), `py/parallel_experiment_runner/planner.py` (`classify_workload`).

# Design

- New terminal status `unknown` (`STATUS_UNKNOWN` in `model.py`), a member of `BREACH_STATUSES` alongside `resource-report-error`: never a hit or a miss, never recorded as a clean calibration sample, and the sweep exits nonzero when any worker is unknown. `UNKNOWN_RESULT_EXIT_CODE = 193` names the wrapping launcher's unknown-result exit code.
- `detached.py`: `resource_report_state` is now a recognised report field, stored on `DetachedResourceReport.report_state` (default `COMPLETE` for reports with no state field, preserving the counter-complete contract). Only `COMPLETE` or `INCOMPLETE` are accepted; any other value is refused as a malformed report (fail-closed).
- `execute.py`: a successfully read `INCOMPLETE` report classifies as `unknown` ahead of the hit/miss decision (runner-observed breach counters still outrank it, as they are measured independently of the producer's Result). The breach line reads `UNKNOWN: required terminal Result unknown (no terminal workload result)`.
- `planner.py`: `classify_workload` returns `unknown` before any hit check when there is no terminal Result: exit code `None`, a negative (signal) code, or 193 — even when 193 is a declared hit exit code or the log matches the hit regex.
- Both copies of the user guide (`py/parallel_experiment_runner/USER_GUIDE.md` and `common/docs/parallel-experiment-runner/USER_GUIDE.md`, kept identical) document the `unknown` status and the `INCOMPLETE` report state.

# Acceptance Criteria

- A test reproduces the exact reported shape (counter-complete report, `resource_report_state=INCOMPLETE`, zero breaches, log matching the hit regex, exit 193) and classifies UNKNOWN, never HIT, and is not recorded as a profile sample.
- An `INCOMPLETE` report classifies UNKNOWN even with a clean exit 0; a `COMPLETE` report (or no state field) classifies as before; an unrecognised state value is a `resource-report-error`.
- Exit 193 classifies UNKNOWN at the planner classifier and at the execute classifier, including when 193 is a declared hit code; a missing (`None`) or negative (signal) exit code with a matching log is UNKNOWN, never HIT or MISS.
- Runner pytest suites pass, strict mypy passes on every changed Python file (tests included), and `scripts/check_client_names.py` passes run bare, never piped.
- Audit of the remaining classifications finds no other non-terminal or unknown result counted as HIT or MISS, or any remaining hole is listed under Notes.

# Notes

Numbered from devbig030's interim block (1400-1469); verified still free on github/main before writing. Audit of the other classifications: breach counters (cpu-timeout, memory-cap, pids-cap, log-cap, wall timeout), cancellation, and malformed/missing detached reports (`resource-report-error`) already precede any hit/miss decision and were already tested; workers that never ran are `cancelled`, not hits. The same hole family fixed beyond the reported shape: missing exit code and signal death reaching the classifier with a matching partial log. No Rust edition of this runner exists (`rs/` has no parallel-experiment-runner crate) and no cross/differential test covers this classification path, so there is no second edition to keep in step; the Python/Rust profile-CSV differential is unaffected because no profile field changed.

## Resolution

[muse watermelon] Fixed in `py/parallel_experiment_runner/model.py` (new `STATUS_UNKNOWN` in `BREACH_STATUSES`, `UNKNOWN_RESULT_EXIT_CODE = 193`), `detached.py` (`resource_report_state` parsed, `COMPLETE`/`INCOMPLETE` only, stored as `report_state`), `execute.py` (`INCOMPLETE` -> `unknown` ahead of hit/miss, with a named UNKNOWN breach line), and `planner.py` (`classify_workload` -> `unknown` for exit 193, `None`, or a signal code before any hit check). Regression tests: the exact reported shape plus clean-exit INCOMPLETE, COMPLETE, and unrecognised-state cases in `py/tests/test_experiment_detached.py`; exit-193 (including declared-as-hit), missing-code, and signal-death cases in `py/tests/test_experiment_planner.py`; an execute-level exit-193-with-matching-log case in `py/tests/test_experiment_cli.py`; `unknown` breach membership in `py/tests/test_experiment_model.py`. Both user-guide copies document the new status. Verified locally: 71 passed in the detached/planner/cli/model suites and 98 passed in the calibrate/profile/wall/limits suites, strict mypy clean on all 8 changed Python files, and `scripts/check_client_names.py` and `scripts/check_issue_parse.py` both pass run bare. Unverified until the contained gate runs (host-quiet window): the full gate-selected validation over this change.
