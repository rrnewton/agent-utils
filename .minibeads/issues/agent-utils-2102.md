---
title: 'wrkslots-rename-in-image-eden-backing: transfer-owner with a new agent name refuses every in-image Eden slot'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T07:45:00-05:00
updated_at: 2026-11-22T07:45:00-05:00
---

# Description

transfer-owner with --to-agent (a rename) runs the clean-tree rebind check, which validates the stored repository path and rejects an in-image Eden backing path ("stored absolute repository path must name one direct sibling of the project root"). A plain same-agent transfer skips that check and passes. So no in-image Eden slot can be handed to a new agent name.

# Design

Make the stored-repository-path rule total over legitimate layouts: accept the in-image backing location the project configuration names, alongside direct siblings of the project root, and keep refusing anything else. Python and Rust editions agree.

# Acceptance Criteria

- Tests: a rename of an in-image Eden slot succeeds; a path that is neither a direct sibling nor the configured backing location is still refused; same-agent transfers unchanged; Python and Rust agree.

# Notes

Reported by a consumer's lead; filed by the coordinator.
