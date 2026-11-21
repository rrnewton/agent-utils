---
title: 'wrkslots-eden-finish-shared-store: Eden finish accounting counts every draft in a shared backing repo'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T13:05:00-05:00
updated_at: 2026-11-21T13:05:00-05:00
---

# Description

A consumer reports that finishing an Eden slot whose checkout shares one backing repository with many other checkouts refuses with "has unaccounted commits: accounting landed=0 open=27 unaccounted=850". The 850 are Sapling drafts made by other checkouts of the same backing store. The Eden finish accounting added for #1702 asks for every `draft()` commit in the store rather than the slot's own, so every finish of a shared-store Eden slot refuses.

# Design

Scope the accounting to the slot: the drafts reachable from the slot's own working-copy parent and bookmarks, plus drafts the slot created (recorded or provable), and never other checkouts' drafts in the shared store. Keep #1702's protection: a slot's own unpushed drafts must still block finish.

# Acceptance Criteria

- A slot sharing a backing store with other checkouts that hold drafts finishes when its own work is accounted for.
- The slot's own unaccounted drafts still refuse finish.
- Deterministic tests with a fake Sapling runner; edition parity where a Rust edition exists.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
