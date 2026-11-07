---
title: 'liaison-first-tab-order: put the owner-facing liaison first in the canonical tab order'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T04:06:27.000000000+00:00
updated_at: 2026-11-07T04:06:27.000000000+00:00
---

# Description

Follow-up to #979 canonical-tab-order. The canonical tab order ranks by
`(role, name, original index)` with a single FRONT role: the designated
coordinator (restore.json `coordinator` / `coordinator.json`), or a launch
profile named `coordinator` / `*-coordinator`, or `liaison` / `*-liaison`;
every other registered agent is a lane and unmanaged tabs come last.

That puts the liaison behind the coordinator whenever both are FRONT: the
liaison commonly runs the `coordinator` profile, so FRONT sorts by name and
the coordinator sorts first. The owner wants the liaison first, then
coordinators and lieutenants, then ephemeral lanes by name, then unmanaged
tabs, repaired continuously.

Wanted:

- Within FRONT, rank the owner-facing agent first. The owner-facing agent is
  the agent a configured chat bridge delivers to (the bridge's `agent_name`,
  discovered from the registry's bridge configuration and state agentctl
  already knows about), or an agent explicitly marked as the liaison (a
  record field `role` of `liaison`, or its launch profile's `role` of
  `liaison`, or a profile named `liaison` / `*-liaison`). Document the
  precedence when more than one source names an agent.
- Then coordinators and lieutenants: the designated coordinator next, then
  other coordinator-profile agents by name. Then lanes by name, then
  unmanaged tabs. The order stays idempotent: a second pass makes no moves.
- A registry with no bridge and no explicit liaison keeps today's behaviour.
- Both editions if the Python edition has the same logic; otherwise document
  the divergence as with #986 restarts.
- Tests on a fake Herdr: a liaison (coordinator profile, bridge target), a
  coordinator, and two lanes in shuffled order come out liaison,
  coordinator, lanes by name; a second pass makes no moves.

Also report (without changing it) how a registry gets the hygiene pass at
all: today only `agentctl watch` runs it, and a registry with only
`watch-agents` never gets it. Say whether the pass should also run from
`watch-agents` or from start/revive/restart/restore, or whether an
`agentctl watch` unit should be added, and recommend one.

## Edition parity note (Python)

The liaison-first ranking is Rust-edition behaviour. The Python edition
has no tab-order or hygiene logic: its `watch`, `restore` and
`watch-agents` commands are refused as Rust-only, so there is no Python
surface to rank tabs with and no differential covers one.
