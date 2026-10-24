---
title: 'suite-a-la-carte: audit whether each tool works alone, and check it periodically'
status: closed
priority: 2
issue_type: task
labels:
- suite
- agentctl
- validation
created_at: 2026-10-24T14:30:41.042245449+00:00
updated_at: 2026-10-24T14:45:52.243681378+00:00
closed_at: 2026-10-24T14:45:52.243681258+00:00
---

# Description

[opus 5.5] The owner wants the suite to be à la carte: a user who installs one tool (team messaging, wrkslots, dagrun, vibe-talk, ...) should not need the rest. Audit every tool's install-time and run-time dependencies with evidence from code, assess whether agentctl (agent management, chat bridge, and planned team messaging in one binary) should be factored, describe standalone-subset scenarios and accidental hard dependencies, propose a periodic standalone check and implement it for one or two subsets, and compare with Gas Town and the other systems in RELATED_WORK.md on composability.

# Acceptance Criteria

ai_docs/transient/2026-10-24-suite-a-la-carte.md lands with the inventory, the dependency graph, the agentctl factoring options and a recommendation, the subset scenarios, the check proposal, and the comparison. scripts/check_standalone.py runs at least two subsets in validation.dag.yaml.

# Notes

[opus 5.5] Delivered: ai_docs/transient/2026-10-24-suite-a-la-carte.md (inventory, dependency graph, agentctl factoring with a recommendation, subset scenarios, comparison) and scripts/check_standalone.py, which runs the tick-hub and wrkslots subsets as packages.standalone.* in validation.dag.yaml. Left as owner decisions in the doc's section 6: the agentctl crate split itself, a public Slack Web API transport, the wrkslots init default, a single-tool setup path, and the next subsets (section 4.3). Unverified: the wrkslots subset's cost on a hosted runner; it was measured only on a busy shared workstation (42 s wall).
