---
title: 'boxed-root-helper-fd-mounts: root helper mounts by resolved fd, verified, with a capped tmp'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-17T18:35:00+00:00
updated_at: 2026-11-17T18:35:00+00:00
---

# Description

[opus 5] Step 3 of #1583 (review 5, before any install): the sealed
root helper in `py/wrkslots/sandbox.py` still mounts some targets by
path string. `_target_fd_path` opens `O_PATH` without `O_NOFOLLOW`
and not via openat2 `RESOLVE_BENEATH` from a verified root, several
mounts and remounts go by name, `_openat2_strict` is check-and-close,
and `_mask` resolves by `realpath` first. A same-uid attacker who can
stop the helper mid-build (SIGSTOP, swap a path component for a
symlink, SIGCONT) can redirect a bind while the helper still holds
CAP_SYS_ADMIN. The namespace is private and same-uid code is not
contained (documented), but an uncaught exception after the binds
must never run interpreter code with the capability held, and the
finished view must be verified against where its mounts landed, not
just that something is mounted there. No sudoers line exists or is
proposed here; install stays an owner decision after this lands and
passes an independent install review.

# Design

- I1 fd mounts (root entry only; the userns entry is unchanged):
  every mount target is resolved once with openat2
  (`RESOLVE_BENEATH|NO_SYMLINKS|NO_MAGICLINKS` from the verified
  `/` fd; missing leaves are created through the resolved parent
  fd), the fd is kept, and the mount is made with
  `open_tree` + `move_mount` (`MOVE_MOUNT_F_EMPTY_PATH|
  MOVE_MOUNT_T_EMPTY_PATH`); flag remounts and propagation changes
  use `mount_setattr` (`AT_EMPTY_PATH`), fresh tmpfs mounts use
  `fsopen`/`fsconfig`/`fsmount` + `move_mount`. Sources stay pinned
  by the fd opened at check time, so swapping the source name after
  resolution cannot change what is mounted. The namespace root
  itself (`/` propagation at build start) is the resolver root and
  stays a mount(2) on `/`. Fail closed: if openat2, open_tree,
  move_mount, mount_setattr, or the fs-mount syscalls are
  unavailable, or any strict mount call fails, the root launch
  refuses; there is no fallback to path-string mounts. Capabilities
  are dropped before any post-build step that can raise, and a build
  failure drops them before the error propagates.
- I1b `_check_root_schema` type-checks every view value (strings,
  string lists, bind pairs, booleans, nullable Eden strings), so a
  non-list `masks` can no longer trip `build_view`'s late assert.
- I2 `_verify_mount_points` checks where binds landed: each bind
  target not also masked must be the topmost mount at its point
  with its source's device; home and `/tmp` still need only exist
  (layered covers change their device). The docstring says exactly
  this.
- I3 The `WRKSLOTS_PDEATHSIG` opt-in is documented as userns-only:
  the kernel clears the death signal when the root helper drops
  uid, so it cannot promise cleanup there. The "killed test leaves
  no boxed child" guarantee for root mode lives in the test harness
  (the live root tests stop their systemd slice in a `finally`).
- I4 Tests per item: fd resolution/symlink refusal, source and
  target swap after resolution (in a throwaway user namespace),
  mount verification (device mismatch refused), `_CLAIM_PREFIXES`
  slot refusal, state-directory (layer parent) refusal, `tmp_size`
  percent form, schema type refusal, and the seal exercised through
  the real root entry (live, installed helper required, skipped
  without one).
- I5 `tmp_size` percent form is capped like the absolute form:
  `N%` of host memory may not exceed 64 GiB. The Rust edition does
  not compute sizes or mount plans (`tmp_size_ok` mirrors only the
  grammar; the spec is enforced by the Python helper for both
  launchers), so no Rust change is needed; that is checked, not
  assumed.

# Acceptance Criteria

- Root-mode mounts are made only through resolved fds; a swapped
  source or target name after resolution cannot redirect a bind
  (user-namespace integration test proves both).
- A bind landing on the wrong device, or missing from the mount
  table, refuses before exec.
- `tmp_size: "100%"` on a host with more than 64 GiB refuses, as do
  non-list/non-string view values, with unit tests.
- Focused tests pass; capped validate and direct gate recorded
  under Notes; independent install review before landing. No
  sudoers line, no install.

# Notes

Security section of #1583 carries the threat model (the attacker is
the invoking user). This step changes no trust decision: root mode
stays sealed with PR_SET_NO_NEW_PRIVS, slot roots stay advisory
with the private namespace as the boundary, and the helper still
refuses to run from a checkout copy without the explicit opt-in.
