---
title: 'agent-teams-coordinator-handover: make coordinator handover unable to silently lose inter-team messages'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-01T08:29:46.328746936+00:00
updated_at: 2026-11-01T11:20:44.969895110+00:00
closed_at: 2026-11-01T11:20:44.969894960+00:00
---

# Description

A fleet silently lost 3 days of inter-team messages (13 messages) when its coordinator coord3 was retired and coord5 took over: the agent-teams declaration still pinned deliverer agent coord3, senders saw only "accepted", `agent-teams coordinators` listed coord5 as live, and 81 `agentctl send coord3` processes hung for a week.

GOAL:
1. Don't pin names: declarations name the registry by default; delivery resolves the registry's designated coordinator at send time. Make that the `init` default; warn on any declaration pinned to an agent name.
2. Red flags in `agent-teams check`, also run on a schedule and surfaced via watch-agents. Flag when the delivery target: isn't live, or is archived or missing; isn't the registry's designated coordinator; has a `managed_by` parent; or when the root of the managed-by tree is some other live agent, or the designated coordinator isn't a root. Every flag says what to run to fix it.
3. Refuse undeliverable sends loudly: send/accept to a missing or archived target answers "undeliverable" with the reason; message stays in sender outbox for retry; receiving coordinator/owner gets a notice. `agentctl send` to a nonexistent agent fails promptly (bounded wait, clear error).
4. A real handover: `agentctl coordinator set NEW` checks every agent-teams declaration naming this registry and fixes or flags any pinned to the old name. Stopping/retiring the designated coordinator refuses without `--handover-to NEW` (or explicit force with warning).
5. Skills: handover checklist (designate new coordinator, check team declarations, drain/forward old inbox, retire old) in the agentctl skill and the coordinator skill in skills/.
Tests for each flag, the refusal, and the handover. Docs: agent-teams and agentctl help and userguides.

Scope limits: no edits to live declarations or live registry designations; test fixtures only.
