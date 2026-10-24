---
title: 'suite-a-la-carte: audit whether each tool works alone, and check it periodically'
status: open
priority: 2
issue_type: task
labels:
- suite
- agentctl
- validation
created_at: 2026-10-24T14:30:41.042245449+00:00
updated_at: 2026-10-24T14:30:41.042245449+00:00
---

# Description

[opus 5.5] The owner wants the suite to be à la carte: a user who installs one tool (team messaging, wrkslots, dagrun, vibe-talk, ...) should not need the rest. Audit every tool's install-time and run-time dependencies with evidence from code, assess whether agentctl (agent management, chat bridge, and planned team messaging in one binary) should be factored, describe standalone-subset scenarios and accidental hard dependencies, propose a periodic standalone check and implement it for one or two subsets, and compare with Gas Town and the other systems in RELATED_WORK.md on composability.

# Acceptance Criteria

ai_docs/transient/2026-10-24-suite-a-la-carte.md lands with the inventory, the dependency graph, the agentctl factoring options and a recommendation, the subset scenarios, the check proposal, and the comparison. scripts/check_standalone.py runs at least two subsets in validation.dag.yaml.
