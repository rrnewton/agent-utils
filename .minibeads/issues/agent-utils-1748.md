---
title: 'standalone-wrkslots-scenario-wall-clock-budget: standalone wrkslots scenario depends on the 60-second remove budget'
status: open
priority: 0
issue_type: bug
assignee: mu-removegaps
labels:
- wrkslots
created_at: 2026-11-16T16:44:12-05:00
updated_at: 2026-11-16T16:44:12-05:00
---

# Description

[watermelon-full-preview] The standalone wrkslots package check fails under host load: its scenario's slot removal exits 3 with a refusal that the remove item used its 60-second budget for work before deletion at the /proc scan after the path fence (budget phase: salvage). Run alone the same removal takes 59-63 seconds, right at the budget, so the gating scenario's result depends on wall-clock timing and host load rather than on what it proves.

# Design

- Make the scenario immune to the wall-clock budget without changing the product's 60-second default: pass `--no-lock-budget` to the scenario's removal, where the scenario's intent allows, or give it a scenario-only budget override.
- Keep whatever the scenario is meant to prove: a whole standalone slot lifecycle (create, commit, handoff, release, read-handoff, remove, status, branch kept), with no session manager.

# Acceptance Criteria

- The standalone wrkslots scenario no longer depends on the 60-second remove budget: its removal carries the scenario-only exemption and the lifecycle assertions are unchanged.
- The product's default 60-second budget is unchanged.
- The standalone check's wrkslots scenario passes on its own terms, without a timing race.

# Notes

Owner rule: a gating scenario must not depend on a wall-clock budget.
