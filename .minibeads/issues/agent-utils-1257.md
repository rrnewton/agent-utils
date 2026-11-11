---
title: 'wrkslots-declared-submodule-local-refs: removal keeps or refuses local branches and tags made inside a declared submodule'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T01:20:00+00:00
updated_at: 2026-11-11T01:20:00+00:00
---

# Description

[opus 5.5] Found while fixing #1255 wrkslots-private-history-refusal. For a submodule its source still declares, a normal (non-drift) removal never computes private history (keep_private_state is false there). Its HEAD and worktree are salvaged, but local branches and tags created inside the slot's submodule store, and not on any remote, are deleted without a refusal or a bundle.

# Design

- Apply the #1255 rule to declared submodules: commits on local refs not covered by the recorded remote's advertised heads or tags are private history.
- The same precise refusal, and the same `--archive-private-history` bundle salvage.
- Keep the declared-submodule fast path when there are no local-only refs (no network call when the refs are all covered locally).
- Rust parity: hand the spec to the Rust remove-parity work, as for #1255.

# Acceptance Criteria

- A declared submodule with a local-only branch or tag refuses with the private-history message, and removes with `--archive-private-history`, keeping a verified bundle.
- A declared submodule whose refs are all covered removes as before.
- Tests count refs and commits; none use time.

# Notes

Follow-up to #1255. Related: #1256 wrkslots-removal-ref-inventory.
