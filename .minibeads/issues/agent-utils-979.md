---
title: 'canonical-tab-order: a canonical tab order in every agentctl-managed workspace, repaired continuously'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-06T20:52:32.232396844+00:00
updated_at: 2026-11-06T21:59:33.819668679+00:00
closed_at: 2026-11-06T21:59:33.819668599+00:00
---

# Description

canonical-tab-order: a canonical tab order in every agentctl-managed workspace, repaired continuously

Owner request (replies 180/181). Canonical order: liaison first, then
coordinators and lieutenants, then ephemeral lanes in a stable order
(by name, documented), unmanaged tabs after. agentctl watch repairs
drift each pass; start/revive/restart/restore place tabs correctly on
create/reuse (Herdr auto-restore returns creation order). Use the
socket tab.list + tab.move as tab_order.rs (#849) does; never move other
workspaces' tabs; idempotent. Role source of truth: launch profile
(coordinator/liaison) per tab_order::is_coordinator_profile, extended
with a liaison profile and the designated-coordinator record; build with
item H (agent-utils-976 workspace-hygiene) on the same census.
