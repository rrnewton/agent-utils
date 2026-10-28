---
title: 'task-tracker-adapters: a two-tier task policy, a generic tracker interface with JSON adapters, and minibeads in the suite'
status: open
priority: 2
issue_type: epic
labels:
- tasks
- design
created_at: 2026-10-28T13:56:51.386024140+00:00
updated_at: 2026-10-28T20:17:06.148736212+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 09:43 ET: GitHub issues are for serious bugs and for reports from other teams and people; task dispatch goes through the local tracker (minibeads here), and a dispatched task is claimed when its agent starts and closed by that agent when its work lands, with the coordinator as backstop. An assessment found the second tier leaking: nothing is ever marked in_progress (0 of 499), real blocking dependencies were unused until 2026-10-28, the coordinator kept a parallel private backlog file instead of filing issues, and GitHub triage had stopped (19 open, 1 closed since 2026-10-14).

Because the suite is a la carte, no tool may depend on minibeads: the owner wants a generic task interface and a JSON configuration that teaches the tools to drive any tracker CLI (minibeads, upstream beads, GitHub, Gas Town, and internal trackers), plus minibeads brought into the suite as a git submodule, on the website and in the tool list.

This epic holds the design (ai_docs/transient/2026-10-28-task-tracker-adapters/README.md, for the owner's review), the near-term fixes as child issues in dependency order, and the minibeads submodule. The bigger features in the design (a normalized task CLI, agentctl and wrkslots task links, native tick-hub task checks) are filed after the owner reviews it.

[opus 5.5] Landed 2026-10-28: ef07c09f (issues, mb 0.29 readability) and 72dae415 (ext/minibeads submodule at v0.29.0, docs, website, skills, and the design in ai_docs/transient/2026-10-28-task-tracker-adapters/). Open for the owner's review of that design (its section 11 lists the questions); the bigger features (normalized task CLI, agentctl and wrkslots task links, native tick-hub task checks, a generic two-tier mirror) are filed after it. The near-term children #632-#636 and #648 are filed in dependency order.
