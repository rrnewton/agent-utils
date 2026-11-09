---
title: 'wrkslots-doctor-identity: doctor reports checkouts whose effective git identity differs from the configured one'
status: closed
priority: 3
issue_type: feature
created_at: 2026-11-08T23:40:00.000000000+00:00
updated_at: 2026-11-09T06:20:00.000000000+00:00
closed_at: 2026-11-09T06:20:00.000000000+00:00
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

## Resolution

Delivered by cl-wrkid, 2026-11-08 ET, in both editions (py/wrkslots/cli.py `_identity_drift_findings`; rs/wrkslotsd/src/doctor_state.rs `identity_drift_findings`, in `doctor-text`).

- For every checkout of this machine's slots, doctor runs `git -C <checkout> config --show-origin --get user.name` and the same for `user.email`, with the caller's environment minus GIT_* variables. Every scope applies, including the user's global configuration, unlike the isolated Git wrkslots runs for itself. Each read is bounded to 10 s.
- A checkout with any difference gets one finding: kind `identity-drift`, scope `row`, `slot`, `slot_type`, `machine`, `checkout`. The detail is `checkout <name>: <key> is '<value>' from <origin>, configured '<expected>'`, `<key> is unset, ...`, or `<key> cannot be read (...)`, with the parts joined by `; `. It comes after the per-row operational findings and before the slot-image findings, in both editions.
- Advisory only: no Git configuration is written. Nothing is reported without a configured `identity`. Submodules are not checked; the design asks for one line per checkout.
- Tests (py/wrkslots/tests/test_doctor_audit_cli_rust.py, Python and Rust doctor JSON must agree), each run with an isolated HOME:
  - a matching worktree config prints nothing;
  - no `identity` configured prints nothing;
  - a worktree override names its `config.worktree` origin;
  - a global fall-through names `$HOME/.gitconfig`;
  - a shared-config write names the shared `.git/config`;
  - an unset identity names both keys.
- User guide: paragraph in "Git identity in new slots".

