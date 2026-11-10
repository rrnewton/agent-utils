---
title: "merge-queue-scratch-root: configurable scratch filesystem, mount guards in shipped units, and cleanup on cancel"
status: open
priority: 3
issue_type: feature
created_at: 2026-11-10T01:50:00.000000000+00:00
updated_at: 2026-11-10T01:50:00.000000000+00:00
---

# Description

A consumer's merge queue builds cold in `<state>/scratch/slot-0` on the host's main filesystem: about 20 GiB on
disk per run (57 GiB by du). On 2026-11-09 this drove the host's shared data filesystem down to 494 GiB free. One
withdrawn run left its 20 GiB build tree behind until someone deleted it by hand. The consumer worked around it
operationally (a bind mount of an image subvolume over the scratch directory, plus a `findmnt` ExecStartPre in the
unit). merge-queue should support this directly.

# Design

1. **scratch_root in the merge-queue config:** where slot checkouts and their build output live (today always
   under `<state>/scratch`). A queue can then put scratch on a separate filesystem without a bind mount. If
   scratch_root is configured but not a mount point (or the configured filesystem is absent), `serve` and the
   donor refuse to start, with a message naming the path.
2. **Mount guards in the shipped unit templates:** the merge-queue serve/donor units that agentctl install-units
   writes, and the guide's examples, get an optional `ExecStartPre=findmnt --mountpoint <path>` guard for each
   configured mount (scratch_root, a worktree image). The service then fails fast instead of writing to the
   filesystem underneath.
3. **Cleanup on withdrawal/cancel:** withdrawing or cancelling an in-flight run runs the validator's cleanup, or a
   configured `on_cancel` command, so the run's build tree does not stay on disk. Record the cleanup's outcome
   with the run.

# Acceptance Criteria

- A config with scratch_root pointing at a separate mount: slot checkouts and build output land there. With the
  mount absent, serve and the donor refuse to start and name the path.
- Generated unit files contain a findmnt ExecStartPre per configured mount when the option is set, and none when
  it is not. Covered by the existing install-units tests.
- Withdrawing a running validation removes (or hands to on_cancel) its build tree. A test asserts the scratch
  directory is gone after the withdrawal, and the cleanup outcome is recorded.
- The user guide documents scratch_root, the guard option and on_cancel.

# Notes

Reported by a consumer harness (2026-11-09), filed for the lead coordinator. Unassigned for now; one lane owns
rs/merge-queue at a time, and the lead decides who.
