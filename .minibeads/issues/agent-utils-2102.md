---
title: 'wrkslots-rename-in-image-eden-backing: transfer-owner with a new agent name refuses every in-image Eden slot'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T07:45:00-05:00
updated_at: 2026-11-23T12:30:00-05:00
closed_at: 2026-11-23T12:30:00-05:00
---

# Description

transfer-owner with --to-agent (a rename) runs the clean-tree rebind check, which validates the stored repository path and rejects an in-image Eden backing path ("stored absolute repository path must name one direct sibling of the project root"). A plain same-agent transfer skips that check and passes. So no in-image Eden slot can be handed to a new agent name.

# Design

Make the stored-repository-path rule total over legitimate layouts: accept the in-image backing location the project configuration names, alongside direct siblings of the project root, and keep refusing anything else. Python and Rust editions agree.

# Acceptance Criteria

- Tests: a rename of an in-image Eden slot succeeds; a path that is neither a direct sibling nor the configured backing location is still refused; same-agent transfers unchanged; Python and Rust agree.

# Notes

Reported by a consumer's lead; filed by the coordinator.

## Resolution

[watermelon-full-preview] Fixed in b6a0e612 on main ("wrkslots: accept the configured in-image Eden backing as a stored repository"). The stored-repository rule in both editions now accepts the exact in-image Eden backing locations the worktrees-image configuration names (`eden_backing_repo` entries under `<mount>/.eden-backing/<suffix>`, derived from `worktrees_image` mount settings), alongside direct siblings of the project root; every other external path is still refused, and a configured backing is exempt from the managed-worktrees refusal. The stored spelling stays absolute, as Eden rows record it, so there is no persisted-format change. Python (`py/wrkslots/cli.py`) and Rust (`rs/wrkslotsd/src/active_state.rs`, `rs/wrkslotsd/src/finish_write.rs`) agree, with unit, rename-gate (`_assert_clean_tree_for_rebind`), and parse-state differential tests. Same-agent transfers are unchanged.
