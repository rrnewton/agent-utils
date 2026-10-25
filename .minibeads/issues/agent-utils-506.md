---
title: 'agent-queue: shared queue crate, coordinator designation and control tree, queue lanes and cancellation'
status: closed
priority: 1
issue_type: feature
labels:
- agentctl
- agent-queue
depends_on:
  agent-utils-496: related
created_at: 2026-10-25T12:42:46.234568456+00:00
updated_at: 2026-10-25T18:59:44.650965638+00:00
closed_at: 2026-10-25T18:59:44.650965517+00:00
---

# Description

[opus 5.5] Implementation of #496 agentctl-team-messaging section 17, phases P0, P0b and P2 (design record ai_docs/transient/2026-10-24-agentctl-team-messaging.md, revision 3).

P0: extract the `agent-queue` crate (rs/agent-queue) from agentctl: queue format, enqueue duplicate check, harness identity and ancestry; agentctl switched to it with no behaviour change; on-disk format documented in the crate.

P0b: per-registry coordinator designation (`agentctl coordinator set|show|clear`); `created_by` (provenance, set once) and `managed_by` (controller, set at start from the proven caller, replaced by adopt, clearable) on agent records, recorded by default with an opt-out. Control is a tree of any depth: `agentctl tree`, watch-agents notifies the nearest `managed_by` (else the designated coordinator), restore/revive and wrkslots hints follow it. The Python edition refuses the new subcommands.

P2: queue lanes and cancellation in the crate and the Rust agentctl: inbox.v2/, cancel/, cancelled/, not_after, pre-paste re-check, bounded drain flags, the 12.4 inventory of queue writers. The Python edition refuses to drain or deliver when new lanes exist.

# Acceptance Criteria

Each phase lands on main green, separately. P0: agentctl tests and the cross differential unchanged and green; crate tests; format documented. P0b and P2 as listed, with tests; Python refusals checked by the cross-edition differential.
