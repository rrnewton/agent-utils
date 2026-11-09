---
title: 'worktrees-image-bind-record-race: bind-record updates are an unlocked read-modify-write with a fixed temp name'
status: closed
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

## Resolution

One locked mutation path in `worktreesimage.py`: `_update_bind_records` takes an exclusive flock on `<control>/.worktrees-image.json.lock` (0600), loads, mutates and writes only when the records changed. The write uses a unique temp name (`.worktrees-image.json.tmp.<pid>.<uuid>`, O_EXCL), fsyncs the file, renames, then fsyncs the directory. `bind_slot`, `unbind_slot`, `relocate_bind`, `save_bind_records` and `save_binds` all funnel through it; readers stay lock-free because the rename is atomic. Tests: six real processes hammering bind/unbind lose no entry and leave no temp file; a source audit proves no unlocked writer remains; unchanged records (including an all-mounted `ensure_binds`) are not rewritten.

Crash ordering is unchanged (mounts outside the lock). Bind (mount, then record): a crash between leaves an unrecorded mount that `ensure_binds` ignores and the mount census counts as foreign use until unmounted; a retried bind records it. Unbind (unmount, then record): a crash between leaves a record saying bound with the subvolume intact, and the next config load re-binds it, so an interrupted remove retries from the bound state. Relocate (unmount, rename, mount, then record): before the rename `ensure_binds` re-binds at the old target; after it, the fenced-sibling guard leaves the slot to recovery. A record that outlives its subvolume is now skipped by `ensure_binds` (test): it fails no config load, re-creates nothing, and blocks no other bind. Rust parity: the Rust worktrees-image handlers must mirror the locked read-modify-write, unique temp name, both fsyncs, the no-write-when-unchanged rule, and the missing-subvolume skip (filed with the landing PUSH).
