---
title: 'wrkslots-detect-worktrees-outside-wrkslots: a read-only detector for worktrees created outside wrkslots'
status: closed
priority: 2
issue_type: feature
labels:
- wrkslots
- doctor
created_at: 2026-11-19T19:40:00-05:00
updated_at: 2026-11-20T06:30:00-05:00
closed_at: 2026-11-20T06:30:00-05:00
---

# Description

[watermelon-full-preview] A consumer asks for "a detector for worktrees created outside wrkslots" (the owner's Nov 18 purge). `reap-stray-worktrees` (#775) reports stray registrations for reaping, but there is no cheap, timer-friendly doctor census that also covers nested worktrees under a slot that wrkslots does not manage.

# Design

- `wrkslots doctor --stray-worktrees` (read-only, deletes nothing): for each managed repository, `git worktree list --porcelain` registrations that are no slot checkout, classified `outside-slot` or `nested-in-slot`; plus nested worktrees under a slot directory whose repository is not managed (a `.git` marker no checkout record names).
- Each row lists path, repository, branch, HEAD, enclosing slot when nested, age (HEAD commit time), bounded size (a walk capped at 20000 entries, `size_truncated` marking the cap; no deep `du` by default) and `has_unpushed_commits` (HEAD contained in no remote branch).
- Human and JSON forms; Rust `doctor-text --stray-worktrees` parity. Runs no other doctor check, so it is cheap enough for a timer. Removes and adopts nothing.
- Tests with a scratch repository holding managed and unmanaged worktrees.

# Acceptance Criteria

- The census reports outside and nested unmanaged worktrees and never a managed checkout.
- Unpushed nested work is flagged; contained work is not.
- Nothing is deleted: every reported worktree still exists after the run.
- Python and Rust forms agree on the reported paths, kinds, branches and unpushed flags.

# Notes

Separate commit from #1929 wrkslots-provenance-only-slot-offline-probe. No persisted state is written by the census.

## Resolution

[watermelon-full-preview] Landed on main in 6e3873a2 (landed as dc2c5fdb): doctor --stray-worktrees read-only census (outside/nested unmanaged worktrees with branch, age, bounded size, unpushed), Rust doctor-text parity, deletes nothing. Focused suites and gate passed (gate reds only waived environment families).
