---
title: 'wrkslots-validate-fresh-submodule-worktreeconfig: never set extensions.worktreeConfig on a shared submodule git dir'
status: closed
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-20T03:25:16-05:00
updated_at: 2026-11-20T06:34:12-05:00
closed_at: 2026-11-20T06:34:12-05:00
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

## Resolution

[watermelon-full-preview] Landed on github/main as f3d6de8c (tree 6e7ae799, replay of 5dd3bb8b over 29b747e7). Root cause: the #1435 guard read `core.worktree` at the moment of the flip; on a submodule's module git dir (`.git/modules/<name>`) Git (re)writes `core.worktree` whenever it connects or updates the submodule (verified against git-submodule: `git submodule update --init` writes it back even after the value was moved out), so a config clean at flip time broke every linked worktree afterwards. Fix: identity (Python and Rust) detects a common dir under `.git/modules/` via `_is_submodule_common_dir` / `is_submodule_common_dir`, never enables `extensions.worktreeConfig` there, and skips that checkout's identity with a logged warning while the create proceeds; the pre-create refusal no longer fires for such repositories. The explicit shared-submodule preparation remains the only path that enables the extension on a module git dir (it moves `core.worktree` first, verified). Doctor's existing `worktree-config-core-worktree-in-common` finding is now covered on a real submodule git dir. No persisted field, journal key, event kind, or `.wrkslots.yml` schema entry was added or changed. Tests: `test_shared_submodules.py` 18/18 (3 new, all failing on github/main), Rust identity parity 3/3, `test_lifecycle.py -k identity` 143 passed; full gate on 5dd3bb8b passed, and the combined tree was re-verified (strict mypy, 19 focused tests, 143 lifecycle identity tests, `cargo test -p wrkslotsd` 208+1) before the overlap landing.
