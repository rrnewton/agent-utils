---
title: 'tests-rewrite-global-gitconfig: a test can rewrite the real global git config'
status: closed
priority: 2
issue_type: bug
labels:
- tests
created_at: 2026-11-12T23:30:00+00:00
updated_at: 2026-11-13T07:30:00+00:00
closed_at: 2026-11-13T07:30:00+00:00
---

# Description

[opus 5.5] A test or tool wrote a throwaway identity (`user.email` set to a
placeholder address) into the operator's real global git config on
2026-11-10, and every later rewrite of that file carried the bad value. The
15:00 rewrite first blamed on the test batch turned out to be a dotfile sync
copying the file into place (identical content, new inode), which is
legitimate. The Nov 10 identity write itself was not found in this
repository: the string appears in no revision on any branch, and the commits
landed in the window contain no identity write. The writer may have been an
unlanded lane run or a tool outside this repository.

Whether or not the original writer is ever identified, the suites must make
the accident impossible and self-reporting: no test session may read or
write the operator's real global git config, and a session that somehow
changes its content must fail.

# Acceptance Criteria

- Both pytest suites (`py/tests`, `py/wrkslots/tests`) run with
  `GIT_CONFIG_GLOBAL` pointing at a per-session temporary file and
  `GIT_CONFIG_NOSYSTEM=1`, installed at session start so subprocesses
  inherit the redirect.
- A session whose real `~/.gitconfig` changes content (created, deleted, or
  different bytes) between session start and finish fails; an inode/mtime
  change with identical content (a sync copy) only warns.
- The same redirect reaches shard workers (`scripts/run_pytest_shard.py`)
  and every validation-graph step including the Rust test harnesses
  (`scripts/validate.py` environment).
- Regression tests prove a global-scope write lands in the session sentinel
  with the real file untouched, and that a subprocess session fails when a
  test writes the real file directly.
- The full gate passes.

# Design

- `py/gitconfig_guard.py`: `GlobalGitConfigGuard.start()` fingerprints the
  real file (existence, content sha256, inode, mtime) and points
  `GIT_CONFIG_GLOBAL` at a fresh empty per-session temp file;
  `check()` classifies the end state as ok / warn (identical content, new
  inode or mtime) / fail (content changed); `sentinel_note()` reports any
  redirected write so the writer can be found.
- Each suite's `conftest.py` starts the guard in `pytest_sessionstart`,
  fails the session in `pytest_sessionfinish` on a content change, and
  prints guard findings in `pytest_terminal_summary`.
- `scripts/run_pytest_shard.py` hands workers a guarded environment
  (caller-provided `GIT_CONFIG_GLOBAL` is kept), and
  `scripts/validate.py` adds the same keys to the graph environment, which
  is how the Rust harnesses receive them.
- Identity for commits stays where it belongs: repo-local config or
  `git -c` flags, as the existing fixtures already do.

# Notes

History search (2026-11-10 11:56-13:20 UTC window, all branches): the
placeholder address appears in no revision (`git log -S` empty, tree grep
empty); the eight landed commits in the window are issue filings and design
notes with no identity write. Reported to the coordinator as "not found in
agent-utils" before the guard landed.
