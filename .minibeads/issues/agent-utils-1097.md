---
title: 'wrkslots-doctor-identity: doctor reports checkouts whose effective git identity differs from the configured one'
status: open
priority: 3
issue_type: feature
created_at: 2026-11-08T23:40:00.000000000+00:00
updated_at: 2026-11-08T23:40:00.000000000+00:00
---

# Description

Once #1096 wrkslots-slot-identity exists, drift still happens: an agent can override the identity, and slots created before the field existed don't have it. Nothing reports it.

# Design

`wrkslots doctor` prints one line per checkout whose effective `user.email` (and `user.name`), resolved across all scopes, differs from the configured `identity`. Each line names the slot, the checkout, the effective value and its origin (as `git config --show-origin` reports it). Advisory only; doctor never rewrites config. Silent when no identity is configured.

# Acceptance Criteria

- Tests: a matching slot prints nothing; a slot with an overriding worktree config, a global fall-through, or a shared-config write each print one line naming the origin.
- No identity configured means no output.
- Rust parity, or the gap recorded.

# Notes

Requested by the lead (devbig014), 2026-11-08. Depends on #1096.
