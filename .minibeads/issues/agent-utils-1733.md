---
title: 'wrkslots-sandbox-pins-validate-binds: slot binds are shared and propagate into foreign mount namespaces, pinning removals'
status: closed
priority: 0
issue_type: bug
assignee: mu-bindprivate
labels:
- wrkslots
created_at: 2026-11-16T19:57:00+00:00
updated_at: 2026-11-16T19:57:00+00:00
---

# Description

[watermelon-full-preview] Validation-slot removals fail with `umount: target is busy` most of the time on a gate host. Any process that starts a private mount namespace while a slot bind exists gets a copy of that bind and keeps it: first a sandboxed agent lane's namespace (dozens of binds pinned), now a system crash-reporting service that restarts about every minute with strict system protection and a private tmp, plus the host-named service. Only a small minority of removals land in a gap between those namespaces starting.

Slot binds are shared today: the image's subvolume bind at the slot path and its peer view through the mirrored home tree share one peer group, so mounts and unmounts propagate into those foreign namespaces. A hand test showed that making both views private and then unmounting privately leaves the slot fully unmounted with no busy error. But a later batch removal re-bound the recorded-but-unmounted slot on config load (the crash-healing re-establish step), shared again; the new mount propagated into the service's fresh namespaces and the removal's own unmount hit the busy error.

# Design

Four parts, in both editions where both implement image binds:

1. Create slot binds (validation and agent slots, both views including the peer mirror) as private from the start; use slave only where something legitimately needs propagation inward, and justify that case in code.
2. In remove, after the fence and before unmount, make the slot's binds private, so an unmount cannot be pinned by copies handed to namespaces that started earlier.
3. On a busy unmount, retry a bounded number of times with backoff; if it still fails, name the process IDs and units whose mount namespaces still hold the mount (by scanning each process's mount table) instead of a bare `target is busy`.
4. The crash-healing re-establish step on config load must not re-bind a slot that is being removed or whose removal is journaled or fenced.

Migration: existing shared binds become private at the next re-establish or remove, with no operator step.

Tests use the existing stubbed mount runner only: never real mounts of live slots and never privileged commands against real paths. Read-only toward the reporting lane's registry and slots.

# Acceptance Criteria

- A stubbed-runner fixture with a shared bind and a foreign namespace holding a copy fences and removes cleanly in both editions; before the fix it reproduces the busy refusal.
- New binds are created private on every view, and a pre-existing shared bind is made private by the next re-establish or remove.
- A busy unmount that never clears names the holding process IDs and units; a busy unmount that clears on retry succeeds without that diagnostic.
- Config load does not re-bind a slot with an outstanding removal journal or a fenced sibling.
- The gate passes and the fix lands on `main` for deployment by the coordinator.

# Notes

## Resolution

[watermelon-full-preview] Landed on `main` as f4644343. Binds are created private on every peer view in both editions, removal privatises after the fence and retries a busy unmount a bounded number of times before naming the holding PIDs and units, config-load healing skips journaled or fenced removals, and shared binds left by older versions become private at the next mount or removal. Regression tests use the stubbed mount runner only (Python `test_worktrees_image.py`, Rust `worktrees_image` unit tests); the full gate passed at owner priority. Not verified live: no live slot was mounted, unmounted, or removed; the reporting host deploys through its coordinator.
