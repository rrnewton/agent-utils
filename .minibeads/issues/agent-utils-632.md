---
title: 'task-dispatch-protocol: file, claim and close every dispatched task in the local tracker'
status: open
priority: 1
issue_type: task
labels:
- tasks
- coordinator
depends_on:
  agent-utils-631: parent-child
created_at: 2026-10-28T13:57:12.427703599+00:00
updated_at: 2026-10-28T14:20:48.539766986+00:00
---

# Description

[opus 5.5] Part of #631 task-tracker-adapters (near-term fix 1). Today a coordinator dispatches from its own notes and files an issue only sometimes, nothing marks in_progress, and closing depends on the worker remembering.

Wanted, with no new code:
1. Before agentctl start, the coordinator makes sure the task has a local-tracker issue (files one if not) and claims it in the agent's name: mb update ID --claim --as <agent> (status in_progress, assignee, lease).
2. The brief names the issue as #<number> <slug> and says: close it yourself with mb close ID --reason 'landed <commit>' when the work is on main, or leave a comment saying what remains.
3. wrkslots create --task carries the slug, and the purpose line carries the issue id.
4. At stop, the coordinator checks the issue is closed (or explains why not in a comment) before removing the slot; it is the backstop, not the closer of record.
5. Write this into skills/agent-utils-coordinator/SKILL.md (generic wording: 'the project's task tracker', with minibeads as the example) and into this harness's brief template.

Done when the coordinator skill says it, the brief template carries the issue line, and a week of dispatches shows every running agent's issue in_progress (mb list -s in_progress).

[opus 5.5] Partly delivered by #631 task-tracker-adapters' change: skills/agent-utils-coordinator/SKILL.md now tells a coordinator that has minibeads installed to file, claim (mb update ID --claim --as NAME), name and have the worker close each task. Remaining: the generic wording for a project with another tracker, the harness brief template, and the stop-time check (items 2-4).
