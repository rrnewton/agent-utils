---
title: 'task-tracker-adapters: a two-tier task policy, a generic tracker interface with JSON adapters, and minibeads in the suite'
status: in_progress
priority: 2
issue_type: epic
assignee: cl-tasks
labels:
- tasks
- design
created_at: 2026-10-28T13:56:51.386024140+00:00
updated_at: 2026-10-28T13:59:19.589134861+00:00
claimed_at: 2026-10-28T13:59:19.589134861+00:00
claimed_until: 2026-10-29T01:59:19.589015732+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 09:43 ET: GitHub issues are for serious bugs and for reports from other teams and people; task dispatch goes through the local tracker (minibeads here), and a dispatched task is claimed when its agent starts and closed by that agent when its work lands, with the coordinator as backstop. An assessment found the second tier leaking: nothing is ever marked in_progress (0 of 499), real blocking dependencies were unused until 2026-10-28, the coordinator kept a parallel private backlog file instead of filing issues, and GitHub triage had stopped (19 open, 1 closed since 2026-10-14).

Because the suite is a la carte, no tool may depend on minibeads: the owner wants a generic task interface and a JSON configuration that teaches the tools to drive any tracker CLI (minibeads, upstream beads, GitHub, Gas Town, and internal trackers), plus minibeads brought into the suite as a git submodule, on the website and in the tool list.

This epic holds the design (ai_docs/transient/2026-10-28-task-tracker-adapters/README.md, for the owner's review), the near-term fixes as child issues in dependency order, and the minibeads submodule. The bigger features in the design (a normalized task CLI, agentctl and wrkslots task links, native tick-hub task checks) are filed after the owner reviews it.
