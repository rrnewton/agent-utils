---
title: 'worktrees-image-bind-record-race: bind-record updates are an unlocked read-modify-write with a fixed temp name'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T17:35:00.000000000+00:00
updated_at: 2026-11-09T17:35:00.000000000+00:00
---

# Description

Found by the first real adopter of the worktrees image (a consumer harness on the shared gate host), by reading agent-utils cc487cc3 (`py/wrkslots/worktreesimage.py`, `imagecmd.py`, `cli.py`). Line numbers are at that commit. The project image representation (`worktrees_image`) is one sparse btrfs file mounted at `<control>/.slots-image`, with one subvolume per slot bind-mounted at `worktrees/slots/<name>`. "Original" means the slot directory on the host file system that `wrkslots migrate` copies from.

`bind_slot` (`worktreesimage.py:510`), `unbind_slot` (559) and `relocate_bind` all run `load_bind_records`, modify,
then `save_bind_records` (459). They are called from migrate (which takes no state lock), from create, and from EVERY
config load via `ensure_binds` (568). The temp file is always `.worktrees-image.json.tmp`.

Two concurrent writers can drop each other's entries or clobber the shared temp file. A dropped entry means the slot
is not re-bound after the next reboot: its path shows an empty directory while its data sits in the subvolume. The
adopter works around it by migrating one slot at a time and re-reading the record after each migrate.

# Design

One flock around every record mutation (read, modify and write under the same lock), a unique temp name per
writer, and fsync of the file (and its directory) before and after the rename. Config-load `ensure_binds` must
not rewrite the record when nothing changed.

# Acceptance Criteria

- A concurrency test: N parallel writers (bind/unbind/ensure) never lose an entry and never leave a stray temp file.
- Every record mutation path takes the lock; a test or audit check proves no unlocked writer remains.
- Rust parity filed or done.

# Notes

Order of the worktrees-image set: #1147 (records race), #1148 (silent mount failure), #1149 (migrate original), then #1150 (migrate liveness) and #1151 (help and small items). Rust parity follows in the Rust worktrees-image handlers.
