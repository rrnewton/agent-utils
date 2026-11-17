---
title: 'wrkslots-reverie-only-slot-submodule-hook: post-provision submodule hook runs where there is no working tree'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T19:10:00.000000000+00:00
updated_at: 2026-11-16T19:10:00.000000000+00:00
---

# Description

Reported by a consuming team on 2026-11-16. `wrkslots create` for a
reverie-only agent slot failed in post-provision hook 1/4
(`git -c protocol.file.allow=always submodule update --init
--recursive`): "git-submodule cannot be used without a working
tree". `wrkslots recover --abort-create` cleaned it up. The same
slot selection with a full repository set works.

Suspected cause: the hook runs at the wrong working directory, or
for a repository that a reverie-only slot does not check out, so the
submodule update has no working tree to act on.

# Design

- Find why the hook runs submodule update where there is no working
  tree for that repo selection: which cwd, which checkout set, and
  why the working selection differs.
- Fix so provisioning hooks run in a checked-out working tree for
  the repositories they name, or are skipped with a recorded reason
  when the slot has no such checkout. Never run a submodule update
  in a bare or absent tree.
- Regression test in both editions (Python and Rust create paths).

# Acceptance Criteria

- A reverie-only slot create with the submodule post-provision hook
  succeeds (stubbed/fixture repositories; no real submodule network).
- The hook never runs in a directory without a working tree; a test
  pins the cwd/checkout it receives.
- Python and Rust create agree.

# Notes

Related: post-provision hooks run before the active row is
published; a failed hook is a provisioning failure retained for
`recover`.

## Resolution

`_run_post_provision_hooks` (Python) and the create hook loop in
`rs/wrkslotsd/src/create_write.rs` ran every configured hook in
every created checkout. For an Eden (Sapling) checkout the cwd is
the Eden mount, which has no Git working tree, so a Git hook such
as `git submodule update --init --recursive` fails with "cannot
be used without a working tree"; ambient GIT_DIR-style variables
could likewise retarget a hook's Git away from its checkout.
Both editions now skip hook steps for checkouts that are Eden or
lack a `.git` (progress still advances, so recovery recomputes
the same plan) and drop the ambient Git location variables
(`_SETUP_HOOKS_CLEARED_ENV`) from the hook environment.
Tests: `py/wrkslots/tests/test_hook_worktree.py` (Eden skip with
a failing hook, ambient GIT_DIR scrubbed); Rust create
differentials unchanged and green.
