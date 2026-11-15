---
title: 'ttl-teardown-remove-grade: occupancy and handoff gates, or retire the path'
status: closed
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] The #1568 lifecycle model finds safety
counterexamples on the TTL teardown path alone: since the live/
held guards landed, `_cmd_ttl_teardown` still archives and
`shutil.rmtree`s with no occupancy census (cwd, exe, fds, maps,
mounts, cgroup) and no handoff gate. A dead owner with a leftover
process holding the tree, or an unread handoff, can be torn down
where `remove` would refuse.

# Design

Either bring teardown to remove-grade admission (occupancy absence
plus handoff-read-or-no-handoff, rechecked under the locks), or
retire `ttl-teardown` in favour of scheduled `retire-pending`
with rescueRefs preserved through salvage. Until then the
finisher (#1568) keeps TTL candidates report-only by default.

# Acceptance Criteria

- Model: no safety violation is reachable on any teardown path.
- Fixture: teardown refuses a slot with a live leftover process or
  an unread handoff, by name, changing nothing.
- Dead, unoccupied, unheld slots past the rung still tear down.

## Resolution

[watermelon-full-preview] Fixed in the #1568 landing:
`_assert_ttl_teardown_safe` (handoff-read plus slot-unused census,
on top of the live/held guards) now runs in
`_execute_ttl_slot_teardown` and in `_cmd_ttl_teardown` before
rescue and again under the mutation locks. Regression tests in
`test_ttl_guard.py` (occupied, unread handoff); the lifecycle
model asserts zero safety violations on every path. Nothing was
invoking teardown automatically on the evidence host; the
finisher reports TTL candidates and never applies them.
