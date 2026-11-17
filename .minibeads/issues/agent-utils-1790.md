---
title: 'wrkslots-self-contained-nested-repo-in-cache-disposable: treat a self-contained nested repository in a cache as disposable'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T18:00:00+00:00
updated_at: 2026-11-17T21:05:00+00:00
closed_at: 2026-11-17T21:05:00+00:00
---

# Description

[watermelon-full-preview] A consumer report: `wrkslots remove <validate slot> --validate-complete --census-seconds 900` passed the census, then refused: "REFUSED: cache directory contains nested Git metadata: .../target/<fixture>/.git: not provably disposable: repository has submodule repositories under .git/modules: third_party". `clean-caches` skips it for the same reason. Source: a build fixture that clones a library WITH a submodule into target/. Its submodule gitdirs and worktrees all lie INSIDE the cache; nothing reaches outside it. The failed nightly leaves about 84 GB that cannot be removed.

# Design

Treat a nested repository in a cache as disposable when its gitdir, all its .git/modules gitdirs (recursively) and all its worktrees resolve INSIDE the cache directory, and nothing outside references it (no worktree registration elsewhere pointing in; no alternates or gitfile pointing out; no linked worktrees outside). Keep refusing otherwise, naming the escaping path. Prior art: #309 validate-nested-git-policy and #871 wrkslots-unlisted-module-salvage. No new persisted field is added (a consumer pins an older reader; see #1788 record-format work).

# Acceptance Criteria

Deterministic tests in both editions (Python and Rust): a self-contained repo plus submodule is disposable; a submodule gitdir outside the cache refuses; a worktree registered outside refuses; an alternates file pointing outside refuses. Each refusal names the escaping path.

# Notes

Priority normal-high. Keep the change in the cache-disposability and clean-caches code.

## Resolution

[watermelon-full-preview] Landed as 79a6defe (fix 4824ad44) on main. The completed-validation nested-Git judgment in py/wrkslots/cli.py and its Rust mirror in rs/wrkslotsd/src/audit_state.rs judge containment recursively; deterministic tests in both editions cover the self-contained submodule case and the gitdir-outside, worktree-outside and alternates-outside refusals, each naming the escaping path. No persisted field was added.
