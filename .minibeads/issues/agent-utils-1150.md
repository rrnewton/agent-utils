---
title: 'worktrees-image-migrate-liveness: migrate''s slot-in-use check reads only cwd and ignores holds and registry liveness'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-09T17:35:00.000000000+00:00
updated_at: 2026-11-09T17:35:00.000000000+00:00
---

# Description

Found by the first real adopter of the worktrees image (a consumer harness on the shared gate host), by reading agent-utils cc487cc3 (`py/wrkslots/worktreesimage.py`, `imagecmd.py`, `cli.py`). Line numbers are at that commit. The project image representation (`worktrees_image`) is one sparse btrfs file mounted at `<control>/.slots-image`, with one subvolume per slot bind-mounted at `worktrees/slots/<name>`. "Original" means the slot directory on the host file system that `wrkslots migrate` copies from.

`worktreesimage.py:365 process_inside` checks only `/proc/*/cwd`. It misses processes holding open fds or mmaps,
an exe or root under the slot, and processes in other mount namespaces that reach the slot by another path.
migrate also ignores registry liveness, holds and the slot's cgroup, so it would migrate a HELD slot, or one whose
live agent has its cwd elsewhere.

# Design

Reuse the remove-path census (cwd, exe, root, fds, maps, mount references, recorded cgroup) and the registered
liveness command; refuse held slots and slots with a live owner.

# Acceptance Criteria

- migrate refuses a held slot, a slot with a live owner, and a slot whose only user holds an fd or mmap (tests).

# Notes

Order of the worktrees-image set: #1147 (records race), #1148 (silent mount failure), #1149 (migrate original), then #1150 (migrate liveness) and #1151 (help and small items). Rust parity follows in the Rust worktrees-image handlers.

## Resolution

Covered by f7477253 (#1162) for `migrate-layout`: its layout census already refuses holds, owners not proven dead by the registered liveness authority, and any process-use census hit for every agent slot before switching layouts.

Delivered here for `wrkslots migrate` (both layouts): the whole migration now runs under the mutation locks, and `_assert_migratable` applies the same one-slot bar — the slot must be registered (an unregistered directory refuses, naming the registry row it needs and how to create one), unheld, with an owner proven dead by the registered liveness command, and clear of the process-use census (cwd, exe, root, open files, mappings, mounts, recorded cgroup) that `process_inside`'s cwd-only check used to miss. Tests refuse an unregistered slot end-to-end, a held slot, a live-owner slot, and a slot whose only user holds an open file with its cwd elsewhere; the registered idle-slot migration still succeeds. Rust parity: gate the Rust migrate handlers (#1161) on the same census under the mutation locks, with the same unregistered refusal (filed with the landing PUSH).
