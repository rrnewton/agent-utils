---
title: 'wrkslots-worktreeconfig-core-worktree: never flip extensions.worktreeConfig on a repo whose common config has core.worktree'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T09:40:00+00:00
updated_at: 2026-11-16T09:40:00+00:00
---

# Description

`_apply_slot_git_identity` (py/wrkslots/cli.py, #1096) runs when `configuration.identity` is set. It sets `extensions.worktreeConfig=true` in the repository's shared config, and its docstring called that "an inert switch".

It is not inert for a repository whose common config carries `core.worktree`, as a submodule's module repository does (`core.worktree=../../../<sub>`). Git requires `core.worktree` to move into the main worktree's `config.worktree` once that extension is on. Otherwise every linked worktree of that repo resolves its top-level wrongly, and plain `git status`/`git diff` there is phantom. Reported by another team on this host; it has recurred three times (11-09, 11-11, 11-16), and on 11-09 it nearly produced a ~1400-file mass-deletion commit.

`_prepare_shared_module` (#1206) already does the move correctly: it moves `core.worktree` into `config.worktree`, verified and logged. The identity path did not share that logic, so the two paths could drift.

# Design

- One shared helper in py/wrkslots/cli.py: `_inspect_worktree_config` is the single reader of a repository's worktree-config state (extension on/off, `core.worktree` in the common config, `core.worktree` held in `config.worktree`, `core.bare=true`), and `_ensure_worktree_config_enabled` is the single writer that enables the extension. Both the identity path and `_prepare_shared_submodule_repo` (through `_shared_module_prep_state`) use them, so the two cannot drift again. The writer never flips the extension silently while the common config holds `core.worktree`.
- Per-caller decision when the common config has `core.worktree`: the identity path refuses, the shared-submodule path moves. Justification: identity is a slot-local convenience, so it must not rewrite the primary checkout's worktree location; its refusal names the exact remedy (move the value into the main worktree's `config.worktree`, or remove `identity` from the configuration). Sharing submodules is an explicit request whose purpose is that preparation, so it performs the move, verified by re-reading the state, logged on stderr, and reversible by moving the value back. The "inert" docstring is corrected.
- A doctor finding, `worktree-config-core-worktree-in-common`, for a repository with `extensions.worktreeConfig=true` while `core.worktree` is still in the common config. That is exactly the broken state the other team's operations unset by hand; doctor catches it, with the remedy, before it phantoms a mass deletion again. Read-only detection; no auto-repair in doctor. It is silent on healthy repositories and on #1206-prepared submodule repositories.
- The documented recipe (common/docs/wrkslots/USER_GUIDE.md, the wrkslots skill) now says to check the common config for `core.worktree` before running `git config --worktree user.name ...` in linked or submodule worktrees, and points at the configured identity and at the doctor finding.
- Rust parity: the Rust edition (rs/wrkslotsd, create_write.rs) has an identity writer mirroring Python (`assert_author_supported`/`apply_author`); it gets the same single-reader refactor (`worktree_config_state`, `ensure_worktree_config`) with identical refusal wording. It has no shared-submodule preparation, and its doctor (doctor_state.rs) is snapshot-scope only, with Git-inspecting findings staying in the Python doctor by design, as the existing shared-submodule doctor findings already do. The new doctor finding is therefore Python-only, for that stated reason.

# Acceptance Criteria

- A submodule-style repo whose common config has `core.worktree`: after the identity path runs, `git status` and `git rev-parse --show-toplevel` are unchanged in the main worktree and in a sibling worktree.
- The refuse path names the remedy.
- The doctor finding fires on the broken state and is silent on the healthy and the #1206-prepared states; doctor changes nothing.
- A plain (non-submodule) repo keeps the existing identity behaviour.
- The shared-submodule preparation still moves `core.worktree`, verified and logged, with top levels unchanged.
- Rust identity refusal wording matches Python.

# Notes

Not ours: the other team's validate slot has its own post-provision hook (`git config extensions.worktreeConfig true`). It is a second writer of the extension on their side and is not changed here; the lead reports it to them. The doctor finding above catches the broken state however it was created.

## Resolution

Delivered by mu-wtconfig (devbig030), 2026-11-16.

- py/wrkslots/cli.py: `_inspect_worktree_config` and `_ensure_worktree_config_enabled` are the one reader and writer for worktree-config state. `_assert_git_identity_supported` and the `_apply_slot_git_identity` flip use them in refuse mode (no flip while `core.worktree` or `core.bare=true` is in the common config; refusal names the remedy); `_shared_module_prep_state` and `_prepare_shared_submodule_repo` use them in move mode (value moved into `config.worktree`, re-read and verified, HEAD checked, logged). The submodule identity branch reads the extension through the same reader. The "inert switch" docstring now states the hazard.
- Doctor: `_worktree_config_doctor_findings` reports `worktree-config-core-worktree-in-common` once per broken repository across slot checkouts, source repositories and their populated submodules, with the move-or-unset remedy; read-only, wired into the doctor composition next to the shared-submodule findings.
- Rust: rs/wrkslotsd create_write.rs mirrors the identity half (`worktree_config_state`/`ensure_worktree_config`, same refusal text). No Rust shared-submodule preparation exists, and the Rust doctor stays snapshot-scope, so the doctor finding is Python-only.
- Docs: USER_GUIDE identity section and skills/wrkslots/SKILL.md now say to check the common config for `core.worktree` first and name the doctor finding.
- Tests: py/wrkslots/tests/test_shared_submodules.py covers the refusal leaving top levels and status unchanged in the main and a sibling worktree, the remedy wording, the preparation move leaving top levels unchanged, the doctor finding on the broken state with no doctor write, doctor silence on healthy and #1206-prepared states, and plain-repo identity behaviour.
