---
title: 'eden-create-failure-cleanup: a failed create --eden must not orphan its daemon'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T11:20:00.000000000+00:00
updated_at: 2026-11-16T00:52:59.000000000+00:00
closed_at: 2026-11-16T00:52:59.000000000+00:00
---

# Description

`wrkslots create --eden` on a fresh clone crashed with
`FileNotFoundError` at the redirection retarget `symlink_to` in
`_provision_eden_slot` (py/wrkslots/cli.py). The daemon-derived
redirection target's parent under host scratch does not exist yet on a
fresh clone, and the retarget loop only removed the derived path when
it already existed, so nothing created the parent.

The crash left an unregistered live per-slot Eden daemon and FUSE
mount: no wrkslots row and no create journal, so nothing tracked the
daemon for cleanup.

Finding (sent to the coordinator before coding): the git create path
writes its CREATE journal before provisioning and updates it as it
goes, so every failure after that write is recoverable. The Eden path
called `_provision_eden_slot` before any journal existed and never
wrote one at all, so the daemon and mount were created before the
first registry write. A Refusal after `eden clone` and an unexpected
`OSError` therefore left the same orphan; the `OSError` additionally
escaped the `Refusal` handler in `main` as a raw traceback, while a
Refusal at least printed REFUSED. `eden.provision_checkout` documents
that the caller owns cleanup on failure, but no caller did it.

Related: #1423 eden-daemon-containment (the daemon runs as
`edenfs@<state>.service`; teardown confirms that instance is gone),
#1432 (an Eden-slot recipe on another host; builds on this fix),
#1204 (the original Eden checkout design).

# Design

- Retarget: create the derived target's parent (`mkdir(parents=True)`)
  before the symlink. The existing refusals stay: a non-empty derived
  target is refused, and a target whose resolved path escapes the slot
  after fixup is refused.
- Journal first: `create --eden` validates its assignment and the host
  cap (both write nothing), then writes a `create-eden` journal naming
  the slot path, the config dir and the mount before
  `eden.provision_checkout` starts a daemon. Success writes the ACTIVE
  row and clears the journal.
- Inline rollback: any failure after provisioning starts (Refusal,
  EdenError, or an unexpected exception) stops the slot's own daemon
  and unmounts its mount using only the slot's own `--config-dir`
  inside the slot, verifies the mount is gone from that config dir's
  list, and removes only the storage the create made (the config dir,
  the mount, the in-slot redirection targets, and the derived symlinks
  that point into the slot). Another daemon, mount or config dir is
  never written to.
- Journaled leftover: when the rollback itself cannot finish, the
  journal stays in phase `cleanup-failed` and the command prints the
  exact leftover: config dir, mount, and the `edenfs@<state>.service`
  unit. A retry over that journal points the operator at
  `wrkslots recover --abort-create --slot S` instead of a bare
  slot-path refusal.
- Recovery gets its own branch: a separate classifier
  (`_classify_eden_create_journal`) and abort
  (`_abort_eden_create` / `_recover_eden_create`) keyed on the
  `create-eden` journal kind, kept apart from the git-shaped
  `_recover_create` / `_abort_create`. Eden abort stops and removes
  with the slot's own `--config-dir`, unmounts the slot's mount, then
  removes the slot directory only when it holds nothing beyond what
  the journal names; anything unexpected is refused and left in place
  with the exact leftover printed. Plain `recover` on an incomplete
  Eden journal refuses and names the abort command, because an Eden
  clone cannot be resumed like a git worktree create.
- Scope: Python edition only. The Rust edition has no Eden create
  path (its create never provisions Eden; it only validates Eden row
  markers), so there is nothing to mirror there; a Rust reader meets a
  `create-eden` journal as an unsupported kind and refuses, which is
  fail-closed rather than a misclassification as a git create.

# Acceptance Criteria

- A stubbed-runner test with a fresh-clone layout (derived parent
  missing) fails with `FileNotFoundError` before the fix and passes
  after it.
- Each failure point (fixup rc != 0, escape refusal, a mid-loop
  OSError) leaves the slot daemon stopped and unmounted, or journaled
  for `recover`; a test shows another daemon's config dir is never
  written to, and pre-existing evidence at a derived target is kept.
- `wrkslots recover --abort-create --slot S` aborts a journaled Eden
  failure cleanly; a retry over the journal points at recover.
- A successful `create --eden` registers its row and clears its
  journal.
- No real Eden clones are run for these tests; the eden runner is
  stubbed throughout.

# Notes

## Resolution

Landed d02350d4 (mu-edenfail, gated; renumbered from #1553), with follow-ups 06acf2d6 and 67be9216. create --eden writes a create-eden journal (slot path, config dir, mount) before the daemon starts and creates the derived parent on a fresh clone; any failure after provisioning stops only the slot's own daemon through its own --config-dir; an unfinished rollback stays journaled with the exact leftover (config dir, mount, unit); recover --abort-create --slot handles Eden journals; a retry over a journal points at recover. The Rust edition has no Eden create path and refuses create-eden journals as an unsupported kind. The eden runner is stubbed in every test.
