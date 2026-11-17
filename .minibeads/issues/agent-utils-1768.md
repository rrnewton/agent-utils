---
title: 'wrkslots-post-provision-hook-no-working-tree: a hook git command inherits the caller GIT_DIR and fails without a working tree'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T12:02:52.000000000+00:00
updated_at: 2026-11-18T11:53:34.000000000+00:00
---

# Description

A slot create naming only one secondary repository (not the project
superproject) failed in post-provision hook 1/4 for that checkout. The
hook runs `git -c protocol.file.allow=always submodule update --init
--recursive` in the checkout and died with `fatal: .../git-submodule
cannot be used without a working tree.` Plain `recover` re-ran the hook and refused the same
way; `recover --abort-create` cleaned the slot. Slots naming only the
superproject create fine.

How wrkslots runs the hooks: both editions run each hook with the
checkout as the working directory and add the five `WRKSLOTS_*`
variables, but the hook inherits the invoking process's whole
environment. Every other Git-running path in wrkslots scrubs the
repository-pointing variables first (`_GitVcs._invocation` strips all
`GIT_*`; the `scripts/setup-hooks.sh` installers strip the eight
variables in `_SETUP_HOOKS_CLEARED_ENV`), but the post-provision hook
runner strips none of them. A caller that exports `GIT_DIR` naming a
bare repository (mirror maintenance under a mirrors directory is the
reported shape) or `GIT_WORK_TREE` naming another tree therefore
redirects the hook's Git away from the checkout: `git rev-parse
--is-inside-work-tree` is false there and `git submodule` refuses
with exactly the reported line, reproduced verbatim on this host. A
linked worktree of a bare repository is itself healthy (inside-work-
tree is true and `git submodule` runs), so the checkout registration
is not the defect. A superproject-only slot can survive the same leak
when the leaked `GIT_DIR` names that same non-bare repository, which
is why the failure showed only on the secondary-only slot.

# Design

- In `_run_post_provision_hooks` (Python) and the `create` writer's
  hook runner (Rust), remove the eight repository-pointing variables
  (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`,
  `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`,
  `GIT_NAMESPACE`, `GIT_PREFIX`) from the hook environment, matching
  the setup-hook installers' established set. The hook keeps the
  caller's remaining environment, including any `GIT_CONFIG_*` it
  sets for itself (the reported hook sets `GIT_CONFIG_GLOBAL`
  inline), and still runs with the checkout as working directory.
- Say so in the user guide's post-provision hooks section.
- No change to hook selection, journaling, or recovery: `recover`
  resumes through the same runner, so an interrupted create left by
  the old behaviour completes once the caller re-runs it.

# Acceptance Criteria

- A deterministic lifecycle test creates a slot naming only a
  secondary repository whose hook runs `git submodule update --init`
  against a local submodule fixture (no network), with the caller
  environment poisoned by `GIT_DIR` naming a bare repository and
  `GIT_WORK_TREE` naming another tree: the create succeeds and the
  submodule is populated. The same test fails on the old code with
  the reported `cannot be used without a working tree` refusal.
- A differential test runs the same poisoned submodule hook in both
  editions with identical results.
- The user guide names the cleared variables in the hook contract.

# Notes

Reported by a consumer team's lead against their wrkslots copy; the
consumer hook itself is correct (`git submodule update --init` in the
checkout is the documented hook shape), so no consumer-side change is
needed.

## Resolution

Fixed by #1731 wrkslots-reverie-only-slot-submodule-hook, landed as
7864a9d3: both editions skip post-provision hooks for Eden or
.git-less checkouts and drop the eight repository-pointing variables
(`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`,
`GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`,
`GIT_NAMESPACE`, `GIT_PREFIX`) from the hook environment, matching
the setup-hook installers. This issue's investigation confirmed the
same root cause independently (a leaked bare `GIT_DIR` or foreign
`GIT_WORK_TREE` reproduces the exact `cannot be used without a
working tree` fatal in a healthy linked worktree, while a linked
worktree of a bare repository is itself healthy).

Complement landed with this closure: two deterministic tests on top
of #1731 — a lifecycle regression creating a slot that names only a
secondary repository whose hook runs `git submodule update --init`
against a local submodule fixture with the caller environment
poisoned by a bare `GIT_DIR` and a foreign `GIT_WORK_TREE`, and the
same poisoned submodule hook as a both-editions differential. Both
fail on the pre-#1731 code with the reported refusal and pass on
7864a9d3.
