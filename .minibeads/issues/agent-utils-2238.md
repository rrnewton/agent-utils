---
title: 'wrkslots-ensure-binds-reclaim-aware: ensure_binds never re-binds a checkout a reclaimer is reclaiming'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-25T09:02:00-05:00
updated_at: 2026-11-25T09:02:00-05:00
---

# Description

Every wrkslots config load, read-only commands included, runs `worktreesimage.ensure_binds`, which re-mounts each bind recorded in `<control>/.worktrees-image.json` whose target is not mounted in the caller's own mount namespace. A reclaimer retires a worktrees-image checkout in two steps: unmount its binds, then drop its bind record. wrkslots' own `unbind_slot` and `_remove_slot_directory` do this, and so does a consumer's external checkout reaper. Between the two steps, any wrkslots command run anywhere on the host sees a recorded bind with nothing mounted and binds the checkout again. The new mount then propagates and pins the checkout the reclaimer is removing. A consumer saw this in the field: the record was rewritten with a new mount_id 50 s after the reaper's umount and before its record drop.

The external reaper also drops the record with an unlocked read-pop-save of the JSON file. That races the bind-records flock that every wrkslots writer has held since #2178, so a concurrent bind of another slot can lose its record.

# Design

A per-slot reclaim marker in the control directory, honoured by `ensure_binds` in the same way as the finish journals and validate-batch seals it already skips (#1733). The bind-record drop is done under the bind-records flock.

- Name: `<control>/RECLAIM.<slot>.lock`, beside `.worktrees-image.json`. It uses the `.lock` suffix, not `.journal`. Every reader, older pinned ones included, counts top-level `*.journal` files as registry state (registry identity and read identity), so every marker created or removed during a concurrent mutation's validation round would cost that mutation a round, and a busy refusal after five. Every reader already skips `*.lock` names, as it does `DELETION-LOCK.*.lock` (#1735), whose meaning is also a held flock. An older reader therefore ignores the marker completely.
- Liveness: a held flock, not presence and not a clock. The writer holds `LOCK_EX` on the open marker for the whole reclaim. A reader opens it read-only, without `O_CREAT`, and tests `LOCK_SH | LOCK_NB`:
  - blocked: the marker is live, and the slot is skipped (no bind, no privatise, no record write);
  - acquired: the marker is stale, so the reader releases it at once and treats it as absent;
  - the file is absent: the marker is absent;
  - any other open error: the marker is treated as live, so the reader fails safe and does not re-bind.
  The check is constant-time per recorded slot and needs no directory scan.
- Payload: JSON, for diagnostics only (`kind: "wrkslots-bind-reclaim"`, `version: 1`, `slot`, `pid`, `started_at` in UTC, free-text `writer`). Readers never require it. An empty file is a valid live marker.
- Writer lifecycle:
  1. Before the first umount: open the marker with `O_RDWR | O_CREAT | O_CLOEXEC | O_NOFOLLOW`, mode 0644, flock it `LOCK_EX`, then check that the path still names the locked inode (retry if not). Write the payload and keep the descriptor open.
  2. Unmount the binds, outside any lock. Unmounting is not constant-time work.
  3. Under the bind-records flock (`<control>/.worktrees-image.json.lock`, `LOCK_EX`, bounded wait): load, pop the slot, write atomically (unique temp file, fsync, rename, fsync of the directory), then unlink the marker. Constant-time work only.
  4. Close the descriptor.
  Abort, when the reclaim is abandoned and the record kept: unlink the marker while still holding its flock, then close it. `ensure_binds` may then re-bind as it does today.
- Stale marker (the writer crashed): the kernel drops the flock, so the file is ignored. `ensure_binds` re-binds exactly as it does today, which preserves unbind's documented crash order ("an interrupted remove retries from the bound state"). The next reclaim of that slot takes over the same file. wrkslots never deletes a marker it did not write. `worktrees-image status` lists every marker as live or stale. A live marker has no maximum age, because the writer's liveness is its flock and the unmount retries are bounded by count.
- wrkslots uses the same protocol: `unbind_slot` and `_remove_slot_directory` hold the marker across their unmount-to-forget window. Their own direct re-bind (content beneath the bind) is not refused by their own marker.
- Rust parity: `wrkslotsd worktrees-image mount` (the Rust `ensure_binds` equivalent; it has no config-load bind path) honours live markers, Rust `unbind_slot` writes the marker, and Rust `worktrees-image status` lists markers.

# Acceptance Criteria

- With a live reclaim marker, or the bind-records lock, held for a recorded bind that namespace A unmounted, any read-only wrkslots command whose mount table (namespace B) shows the target unmounted does not re-bind the target and leaves the record untouched. The test uses fakes only and mounts nothing.
- With no marker, or only a stale one, today's re-bind behaviour is unchanged.
- An older reader ignores the marker: registry identity and read identity are unchanged by a `RECLAIM.*.lock` file.
- `unbind_slot` and `_remove_slot_directory` hold the marker across unmount and record drop and remove it afterwards, on success and on abort.
- The Rust `wt_mount` and `unbind_slot` behave the same way, proven by Rust tests.
- The user guide documents the external-reclaimer protocol: marker name, flock, record drop under the records lock, and lifecycle.

# Notes

Related to #2199, which moves unbind's unmount under the records lock. The marker covers the unmount-to-drop window without putting non-constant-time unmount retries under that lock. Root tests stay skipped unless `WRKSLOTS_ALLOW_ROOT_TESTS=1`.
