---
title: 'wrkslots-worktrees-image-host-scope: Rust parity for machines host scoping'
status: open
priority: 1
issue_type: task
created_at: 2026-11-09T21:22:57.000000000+00:00
updated_at: 2026-11-09T21:22:57.000000000+00:00
---

# Description

Python #1152 worktrees-image-host-scope (3f0d04c2) scopes the
worktrees_image section to named hosts. Rust currently refuses
any config using `machines`, so a consumer cannot commit such a
section. Required parity:

- `machines` is validated on every host (non-empty list of unique
  non-empty strings), even hosts it does not name.
- One applies-here decision: an unlisted host behaves exactly as
  if the section were absent (no mount, no bind, no current_state
  call, create guard passes).
- Doctor shows the scope only when configured:
  `worktrees-image: skipped: <reason>` or
  `worktrees-image: applies: state=<state>`, plus the JSON field.
- worktrees-image commands on an unlisted host refuse with
  `worktrees_image does not apply on this host: ...`.

# Design

- mu-rsimg owns `rs/wrkslotsd/src/worktrees_image.rs`
  (settings parser, applies_on, skip reason, the refusal, and
  handler settings plumbing).
- `config_validate.rs`, `config_docs.rs`/`config_convert.rs`, and
  doctor Config/doctor surfaces are shared-core files owned by
  mu-rswrk2; the exact shared edits are proposed through coordb
  and not made unilaterally.
- The later mount_at/quota_mode keys from mu-slotsmount arrive in
  the same parser; keep those blocks separate and do not add a
  strict refusal that would break them.

# Acceptance Criteria

- A config with machines validates identically in both editions
  on listed and unlisted hosts, including malformed machines.
- Rust doctor prints the scope line and JSON field only when a
  worktrees_image section is configured.
- Rust worktrees-image handlers refuse on unlisted hosts with the
  Python wording.
- Oracle differentials cover listed, unlisted, absent, and
  malformed machines.

# Notes

Part of #562 rust-primary-wrkslots. Ahead of #1161
wrkslots-migrate-convert-handlers by coordinator direction.
