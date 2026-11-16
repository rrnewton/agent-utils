---
title: 'wrkslots-eden-mount-outside-git-worktree: mount Eden checkouts outside any git worktree'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T14:10:00-05:00
updated_at: 2026-11-16T14:10:00-05:00
---

# Description

[watermelon-full-preview] Eden slots under a git project checkout are
impossible: `create --eden` mounts the checkout at
`<slot>/checkout` inside the project, and Sapling discovery at
`<project>/worktrees/validate/<slot>/checkout` walks past the mount's
`.hg` symlink to the outer git repo, so the #1555 discovery guard
(correctly) refuses and rolls back (rc 3). Fix (b) from the 2026-11-15
Eden work — mount Eden checkouts outside any git worktree, at a
host-local Eden mount root, while keeping slot records and storage in
the image — was never built, and the validate directory is hard-wired
as `<worktrees root>/validate`.

# Design

- New wrkslots configuration key `eden_mount_root` (documented): an
  absolute, host-local directory outside the project checkout. When
  absent, a per-host default outside the project checkout is derived.
  A value inside the project root refuses at load.
- `create --eden` mounts the checkout at
  `<eden_mount_root>/<slot-type>/<slot>`; the slot directory (record,
  lock, journals, `.eden-state`, in-slot redirection targets) stays
  where it is. The checkout path stored in the record is the mount
  (an absolute path); discovery at the mount resolves to the mount
  itself, so the #1555 guard passes.
- Old in-slot Eden rows (`<slot>/checkout` and the legacy
  `<slot>/<name>` form) stay readable. `remove`, `recover`,
  `doctor`, `archive` and the absent-row paths resolve an Eden
  checkout from its recorded path, whether in-slot or at the mount
  root, and refuse an absolute Eden path anywhere else.
- The Eden mount location no longer derives from the hard-wired
  validate directory; validate and agent Eden slots both mount under
  the mount root.
- Rust parity: the Rust edition recognises and validates the new
  config key and the outside-mount record shape; it keeps refusing
  `create-eden` journals as an unsupported kind (it has no Eden create
  path), which stays fail-closed.
- No real Eden clone or mount is ever run; tests use the stubbed
  runner throughout.

# Acceptance Criteria

- With `eden_mount_root` set outside the project, a stubbed
  `create --eden` registers a row whose checkout path is the outside
  mount, the slot directory stays in place, and the discovery guard
  passes; on the parent code the same create refuses (unknown key /
  in-project mount).
- `remove`, `recover --abort-create`, `doctor` and
  `recover-absent-validate-rows` handle an outside-mount Eden row;
  an Eden row naming an absolute path outside the mount root refuses.
- In-slot Eden rows from before this change still validate, remove
  and archive.
- Deterministic regression tests in both editions where the tool has
  both; condition-based waits only.
