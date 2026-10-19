---
title: 'submodule-test-default-branch: two clone tests fail where git''s default branch is not main'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-10-19T09:35:55.087214621+00:00
updated_at: 2026-10-19T09:37:32.529669475+00:00
closed_at: 2026-10-19T09:37:32.529669335+00:00
---

# Description

[opus 5.5] `py/wrkslots/tests/test_ownerless_clone.py` `test_a_clone_with_an_initialised_submodule_and_worktree_config_is_ready` and `test_a_submodule_initialised_in_a_linked_worktree_is_ready` create the submodule's bare remote with a plain `git init --bare` and push only `main`. Where `init.defaultBranch` is not `main`, the remote's HEAD names an unborn branch and `git submodule add` fails with "You are on a branch yet to be born". The nightly run, which does not set the option (repository-ci does), has failed on it every night since 2026-10-15.

Fix: create that remote with `--initial-branch=main`, as the other wrkslots tests do.

Acceptance: both tests pass with `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=init.defaultBranch GIT_CONFIG_VALUE_0=master` in the environment, and so do both wrkslots component shards. Other suites' default-branch assumptions are unobserved until a full keep-going nightly shows no "branch yet to be born".
