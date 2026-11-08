---
title: 'wrkslots-slot-identity: a configured git identity applied slot-privately to every checkout on create'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-08T23:40:00.000000000+00:00
updated_at: 2026-11-08T23:40:00.000000000+00:00
---

# Description

Agents working in wrkslots slots commit under whatever git identity they fall through to:

- Nested slots fall through to `~/.gitconfig`, which is the owner's personal identity or a host-derived one.
- In flat slots, a lane that runs plain `git config user.name/email` writes the SHARED `.git/config`, which changes the identity for the main checkout and every sibling slot.

An audit of one consumer's public repository found that more than half of the last 200 commits carry an agent identity that embeds an internal host name, against a minority by the consumer's required bot identity. The consumer works around it today with a long `post_provision_hooks` shell literal.

# Design

- A new optional wrkslots config field `identity: {name, email}`. The value lives in the consumer's config; agent-utils ships no default identity.
- On create, per checkout:
  - the slot's root checkout gets `extensions.worktreeConfig=true` plus `git config --worktree user.name/email` (slot-private, never the shared `.git/config`);
  - each initialised submodule checkout gets plain `git -C <sub> config user.name/email` (that slot's private module config).
- The steps are journaled like other create steps and validated (non-empty name, plausible email). A missing submodule path is reported, not fatal to create.
- No field means no change.
- Python first. Mirror in Rust, or record the parity gap for the Rust wrkslots owner in the same landing.

# Acceptance Criteria

- With `identity` configured, a commit made in a new slot's root and in its submodules carries that identity, and the shared `.git/config` and sibling slots are unchanged (tests for flat and nested layouts).
- Without the field, create behaves exactly as before.
- Invalid values are refused at config validation, and a missing submodule is reported.
- Documented in the config reference and the user guide; Rust parity done or recorded.

# Notes

Requested by the lead (devbig014), 2026-11-08, from a consumer's identity audit. Pairs with #1097 wrkslots-doctor-identity.
