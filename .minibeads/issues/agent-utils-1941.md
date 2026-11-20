---
title: 'wrkslots-validate-fresh-submodule-worktreeconfig: never set extensions.worktreeConfig on a shared submodule git dir'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-20T03:25:16-05:00
updated_at: 2026-11-20T03:25:16-05:00
---

# Description

[watermelon-full-preview] A validate-fresh create on a submodule repository sets `extensions.worktreeConfig=true` in the shared submodule git dir (`<project>/.git/modules/<name>/config`). Every linked worktree of that submodule repository then inherits `core.worktree` from that config and can show phantom mass deletions, and the fresh checkout itself can refuse with `git-submodule cannot be used without a working tree`. The earlier guard (#1435 wrkslots-worktreeconfig-core-worktree) refuses to flip the extension while `core.worktree` is in the common config, but this path still flips it: either a different code path, or `core.worktree` is absent at the moment of the flip and Git writes it into the shared submodule config afterwards. A consumer's validations are blocked by this; it has recurred three times. Reader skew: a consumer pins an old reader (writer ceiling 2), so no new persisted field, journal key, or event kind may be added without reporting to the coordinator first.

# Design

- Reproduce in scratch state: a superproject with a submodule whose git dir lives under `.git/modules/<name>` (with `core.worktree` in that config, as Git writes for submodules), plus a validate-fresh create (or the exact create path a validate materialization uses) with a slot identity configured; show the flag gets set and the checkout breaks.
- Fix so wrkslots never sets `extensions.worktreeConfig` on a shared submodule git dir, or on any common config that has `core.worktree`. For identity on such checkouts, use a mechanism that needs no extension (per-command author/committer environment in the commands wrkslots runs, or a per-worktree include Git honours without the extension — verify which works), or skip identity there and log it.
- Keep Rust edition parity if the identity hook exists there too.
- Doctor reports the broken state (`extensions.worktreeConfig=true` plus `core.worktree` in a shared submodule config) with the exact remedy, read-only.

# Acceptance Criteria

- A test with a real submodule layout proves the flag is never set on the shared submodule git dir, the fresh checkout works, and existing linked worktrees are unaffected; at least one test fails on the current code.
- Tests drive the exact defect with condition-based waits only and pass on a host above 75% CPU.
- No new persisted field, journal key, or event kind is added (reader ceiling 2 is pinned by a consumer).

# Notes

[watermelon-full-preview] mu-wtconfig lane, 2026-11-20. Priority high: a consumer's validations are blocked.
