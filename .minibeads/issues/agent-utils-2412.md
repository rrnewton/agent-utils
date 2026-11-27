---
title: 'gate preflight: add repository.check.python-types and vibe-talk.check.scripts as conditional preflight steps, measured, so cheap type checks refuse before queueing'
status: open
priority: 2
issue_type: task
assignee: cl-tmpfix
labels:
- gate
- validation
created_at: 2026-11-27T17:46:00-05:00
updated_at: 2026-11-27T17:46:00-05:00
---

# Description

The 2026-11-27 frequent-red audit (P5) counted full gates that went red only on cheap, deterministic type checks a lane could have run first:

- `repository.check.python-types` (`make mypy`): 21 heads.
- `vibe-talk.check.scripts`: 13 heads.

Each such red costs a whole gate slot.

#2308 already adds `repository.hygiene.issue-parse` and the rustfmt checks to the preflight, conditional on the touched paths. That was the audit's P8: the audit read an outdated checkout, which showed only two preflight steps.

# Design

- Add both steps to gate.py's preflight on #2308's pattern:
  - `repository.check.python-types` when the change touches a `.py` file, an extensionless Python script under `scripts/`, or the mypy configuration;
  - `vibe-talk.check.scripts` when the change touches vibe-talk's Python;
  - both when the change is unknown.
- Measure each step's preflight wall time on a hot host (load1 recorded), cold and with a warm mypy cache. Keep the whole preflight under about 2 minutes, and leave out a step that pushes it past that.
- Refusal hints in `PREFLIGHT_FIX_HINTS`.

# Acceptance Criteria

- A change with a type error refuses at preflight with exit 65 and the step named, before any gate slot is taken.
- A change that touches no Python runs neither step.
- The measured wall times are recorded in this issue.
- Tests sit beside #2308's in `py/tests/test_gate_admission.py`.

# Notes

Filed 2026-11-27 at the lead coordinator's request.
Source: the harness repository's `ai_docs/reports/measurements/2026-11-27-frequent-red-steps-audit.md`, item P5 and steps 9 and 18.
