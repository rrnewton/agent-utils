---
title: 'eden-discovery-guard: an Eden mount must discover as itself, never as the outer repo'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T16:45:00.000000000+00:00
updated_at: 2026-11-15T16:45:00.000000000+00:00
---

# Description

When a slot lives inside a project that is itself a Git/Sapling
working tree, `sl --cwd <slot>/checkout root` returned the OUTER
repo, so an `sl goto` meant for the Eden checkout moved the shared
outer checkout. `sl -R <mount>` resolved correctly every time. The
mount's `.hg` is a symlink into `.eden-state/clients/<name>/`, and
`--cwd` discovery walked past it to the outer working tree.

Any Eden recipe (including the host recipe another lane is writing
from #1432) must make wrong discovery impossible, not merely
unlikely. That recipe should also keep the mount outside any other
working tree where the layout allows it; this issue records that
requirement for it.

Related: #1558 (Eden create journal and rollback, which this guard
reuses), #1432 (Eden-slot recipe on another host).

# Design

- Create-time proof: after provisioning, and again before the ACTIVE
  row is written, `sl --cwd <mount> root` must equal the mount.
  Otherwise create refuses and runs the #1558 cleanup (stop the
  slot's own daemon, remove what the create made, or journal the
  leftover). Where the mount carries a `.git` of its own, the same
  equality is required of `git -C <mount> rev-parse --show-toplevel`.
- Re-check: `doctor` flags any live Eden row whose mount no longer
  discovers as itself, and `status` records the discovery result on
  Eden rows, so a mount that went stale after create is visible
  before anyone mutates through it.
- Workaround until this lands (and the standing rule after): use
  `sl -R <mount>` / `git -C <mount>`, never bare `--cwd` discovery,
  and assert the root before any mutating command.

# Acceptance Criteria

- A stubbed-runner fixture whose `sl root` reports a parent directory
  makes create refuse and roll back (daemon stopped, storage removed
  or journaled).
- Doctor flags a live Eden row whose reported root differs from its
  mount; a matching root produces no finding.
- Tests use the stubbed runner throughout; no real Eden is touched.

# Notes

Resolution: (fill in at landing, including why discovery fell
through to the outer repo)
