---
title: 'validate-selection-narrow: a change to one component selects only that component, proved per component'
status: open
priority: 1
issue_type: task
assignee: mu-testaudit
labels:
- validate
- test-robustness
created_at: 2026-11-10T22:46:59+00:00
updated_at: 2026-11-10T22:46:59+00:00
---

# Description

[muse watermelon] Owner, reply 346 (2026-11-10): "AND there should be good test selection so we are not running tests for agent-util A when only B is modified."

`scripts/validate.py` already maps paths to groups (`PREFIX_RULES`, longest prefix wins) and components (`validation/components.json`), with the fail-safe that an unclassified path selects everything. Over-broad entries remain: `scripts/validate.py`, `validation.dag.yaml`, `validation/`, `common/bin/`, `Makefile`, `setup`, `.github/` and `rs/bin/cargo-runner` each select every group, and the `repository-infrastructure` component owns the validation graph and scripts, so infrastructure edits drag in broad sweeps. Some of that breadth is real (the graph definition, a shared launcher); some is unexamined. This issue narrows what can be narrowed safely and proves the result: for a sample change per component, a maintained test asserts exactly which groups, components and steps get selected, including that component A's sample never selects component B's steps.

Gates currently run `validate.py --all`, so narrowing pays off first in the edit-run loop. This issue also records what would make path selection trustworthy as the default gate (picked up by the gate-speed lane's sharding work): the per-component selection test green, the unclassified-path fail-safe intact, disabled-step accounting named in every plan, and a periodic `--all --cross-all` full-contract run (nightly or release) catching anything selection missed.

# Design

- Audit each select-all rule: keep it only with a comment naming what observably breaks across components (as `rs/bin/cargo-runner` already does); otherwise narrow to the groups and components that can observe the edit. `validation/` in particular splits by fragment where a fragment maps to one component.
- Selection test: extend the offline `--self-test` expectations (or the validation graph contract test) with one sample path per component asserting selected groups and components, plus negative assertions (sample for B selects no `component-A` step). The test runs in hygiene on every change.
- Fail-safe unchanged: a path with no prefix rule and no owning component still selects everything and names itself as the reason; a rename still selects for both source and destination.

# Acceptance Criteria

- The per-component selection test passes and fails when a rule is deliberately widened (negative control recorded).
- Each remaining select-all rule carries a comment justifying its breadth; each narrowed rule's before/after selection for its sample paths is recorded in Notes.
- `validate.py --self-test` and the validation graph contract tests pass; no previously selected check is dropped for a path that can observably break it.

# Notes

Filed by the test-robustness audit lane (mu-testaudit) on a normal gate (only the disables land at owner priority). Coordinate `scripts/validate.py` edits with the gate-speed lane (Part 0 landed as ea3e58cb; later parts touch the same file) and the cross-robustness lane.
- Audit + proof (mu-testaudit): per-component selection is already narrow at the component layer; `test_per_component_selection_is_narrow` in `py/tests/test_validation_graph_contract.py` now proves it for agentctl, wrkslots, wrkviz/timeline and vibe-talk samples (each selects its own steps, none of an unrelated component's). Remaining select-all rules audited and kept with their stated reasons: `scripts/validate.py`, `validation.dag.yaml`, `validation/` (they define the planner/graph itself; the graph contract tests prove a malformed fragment red), `rs/bin/cargo-runner`, `common/bin/`, `Makefile`, `setup` (shared launchers every component dispatches through). Narrowing those further needs a fragment-to-component graph split, recorded here as the remaining work.
