---
title: 'wrkslots-create-skip-post-provision-hooks: create can opt out of configured post-provision hooks, with a recorded reason'
status: open
priority: 3
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-24T17:16:00-05:00
updated_at: 2026-11-24T17:16:00-05:00
---

# Description

A project's configured post-provision hooks (for example a submodule update, or a dependency regeneration that runs cargo and vendoring tools) run at every `wrkslots create`, and there's no way to opt out, not even with `--offline-reason`. A slot created for a network-disciplined or ordered measurement therefore does cargo and network work before the measurement's own steps run, and the measurement has to record that as a deviation. A consumer team hit this on 2026-11-24.

# Design

- `wrkslots create --skip-post-provision-hooks REASON` creates the slot without running any configured post-provision hook. The reason is required and non-empty.
- The slot-created event records that the hooks were skipped, and why, so audits and later readers can see the slot isn't fully provisioned.
- `wrkslots status` (and the slot's detail view) show that the hooks were skipped, with the reason.
- Optional, for discussion: `--offline-reason` could also skip hooks a project marks as network-touching (a per-hook `network: true` key in the project config), rather than all of them.
- A skipped slot can later run its hooks on request (for example `wrkslots provision-hooks <slot>`), so it isn't stuck half-provisioned.

# Acceptance Criteria

- Test: create with `--skip-post-provision-hooks "measurement"` runs no hook, and the event records `post_provision_hooks: skipped` with the reason.
- Test: a create without the flag runs the hooks as today.
- Test: an empty reason is refused.
- The new event field is listed to consumers before landing (pinned older readers).

# Notes

Requested by a consumer team's lead on 2026-11-24 (low priority).
