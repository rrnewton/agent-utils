---
title: 'wrkslots-register-nested-worktrees: record extra linked worktrees of the slot''s own repository'
status: open
priority: 2
issue_type: feature
assignee: mu-wrkslots
created_at: 2026-11-06T22:36:35.000000000+00:00
updated_at: 2026-11-06T22:36:35.000000000+00:00
---

# Description

There is no way to record additional linked worktrees of a slot's own repository. `import-existing --repo a=repo --repo b=repo` refuses with "flat layout requires exactly one repository per slot", and `register` follows the same one-repo-per-flat-slot model. Consumer lanes create such worktrees (nested under `ignored/`) for side work, so leak and unpushed diagnostics, and removal, cannot account for them.

# Design

- Let a flat slot record nested linked worktrees of its own repository as extra checkout rows (path, branch or detached head), via `register`/`import-existing` or a new `add-worktree` verb.
- Leak, unpushed and removal checks cover those rows: salvage publishes their unpushed work, and removal removes or refuses them explicitly.

# Acceptance Criteria

- Tests: register a nested linked worktree; status and audit list it; salvage publishes its unpushed commits; removal handles it.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer lane. Related: #982.
