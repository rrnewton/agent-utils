---
title: 'wrkslots-failed-create-leaves-worktreeconfig: a failed create must not leave extensions.worktreeconfig in a shared submodule common config'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-20T19:00:00+00:00
updated_at: 2026-11-20T19:00:00+00:00
---

# Description

[watermelon-full-preview] Relayed 2026-11-20: a FAILED `wrkslots create --repo <submodule>=<submodule>` left `extensions.worktreeconfig` set in a SHARED submodule's common config. With `core.worktree` also set there, every linked worktree of that submodule then reads as a mass deletion. It was contained by hand (backup, one-line unset, failed creates aborted); it had happened before on another host.

# Design

A failed create must never leave the shared submodule common config modified: set worktree config per-worktree only, or record and roll back every common-config change on any create failure, and refuse to leave the shared common config modified. Separately, post-provision hook 4 should SKIP a repository that has no generator instead of failing the create, and name the skip in the create report. No new persisted field is introduced; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- A test where a create fails after the config step asserts the shared submodule's common config is byte-identical afterwards.
- Post-provision hook 4 skips a repository with no generator and names the skip in the create report.
- Existing create/submodule suites pass, in both editions where the surface exists.

# Notes

Queued after #1876 wrkslots-remote-proof-without-egress. Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.

Note: a crash mid-restore can leave a stale `config.lock`/`config.worktree.lock` or a `.config.wrkslots-restore` temp file in the module common dir; both are named in the rollback output and are safe to remove by hand. The original incident came from a create on a submodule, which #1941 already handles in `_apply_slot_git_identity`; the third `worktree add` path (the direct share-submodules command) is out of scope for this create-rollback change.
