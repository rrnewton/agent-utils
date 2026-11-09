---
title: 'post-provision-hook-slot-types: post-provision hooks cannot be limited to slot types and do not see the slot type'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-09T11:00:09.000000000+00:00
updated_at: 2026-11-09T11:00:09.000000000+00:00
---

# Description

_run_post_provision_hooks runs every configured hook for every checkout, and the
hook environment (WRKSLOTS_CHECKOUT, WRKSLOTS_CHECKOUT_NAME, ...) carries no slot
type. A consumer wants a dependency-generator hook only in agent slots, never in
validation checkouts. Today its hook has to guess from the WRKSLOTS_CHECKOUT path,
which is fragile.

# Design

- Add an optional per-hook slot_types: list in the config (for example
  [agent]). A hook with no slot_types runs for every slot type, as today.
  Reject an unknown slot type at config load with a clear error.
- Add WRKSLOTS_SLOT_TYPE to the hook environment, for every hook.
- Document both in the user guide, with the config example and the variable.
- Keep older readers working: if the new key needs #1109's ignorable_keys or
  min_wrkslots_version to stay forward compatible, use it and say which.

# Acceptance Criteria

- A hook with slot_types [agent] runs for agent slots and not for validation
  checkouts.
- A hook without slot_types runs everywhere, as before.
- Every hook sees WRKSLOTS_SLOT_TYPE.
- An unknown slot type is refused at config load.
- Python first; Rust parity (mu-rswrk2, devbig014) where Rust runs hooks.

# Notes

Requested by a consumer via the lead, 2026-11-09. Filed by the devbig030
coordinator.
