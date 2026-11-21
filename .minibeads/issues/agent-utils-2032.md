---
title: 'agent-teams-health-test-flakes: health tests must be condition-based and robust'
status: open
priority: 2
issue_type: bug
labels:
- agent-teams
created_at: 2026-11-21T03:05:00-05:00
updated_at: 2026-11-21T03:05:00-05:00
---

# Description

[watermelon-full-preview] Three agent-teams health tests flake on clean main, rotating per run: pinned_designation_is_flagged_for_queue_deliverers, unreachable_orchestrator_is_distinct_and_still_deliverable, pinned_agent_is_a_warning_and_missing_target_an_error. Tests must be condition-based and robust (rule 346). Issue file only; do not fix on the teamsdeliver branch.
