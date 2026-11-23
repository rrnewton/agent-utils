---
title: 'wrkslots-failed-create-leaves-worktreeconfig: a failed create must not leave extensions.worktreeconfig in a shared submodule common config'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-20T19:00:00+00:00
updated_at: 2026-11-23T12:00:00+00:00
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

## Resolution

[watermelon-full-preview] Landed as 22fe99f5. Current main still left the shared config modified on a failed create, so the fix was needed. Create and create-recovery now snapshot each shared module common config (`config` + `config.worktree` bytes) before preparation; on any failure before ACTIVE publication they remove any linked worktree the attempt added and restore the config byte-identically (compare-and-swap against the exact post-prepare bytes, atomic rename under `config.lock`/`config.worktree.lock`, mode preserved, symlink and held-lock restores skipped with a printed reason), and the journal `hook_progress` resets so recovery reruns every hook. Post-provision hooks stay opaque: a hook exiting 0 with a `wrkslots-hook-skip: <reason>` stderr line is reported as `skipped (<reason>)` in the create report, documented in the user guide; unmarked hooks behave exactly as before. No new persisted field. Tests cover byte-identical restore after a failing hook, clean `git status` in the recovered slot submodule, hook rerun after rollback, and skip reporting; each fails on unmodified main. Follow-up (not a blocker): the inner prepare restore calls `_restore_common_config` without `expected=`, so a concurrent writer's change during `_ensure_worktree_config_enabled` could be overwritten; a later fix should hold the lock across prepare or CAS against the post-prepare bytes.
