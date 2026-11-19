---
title: 'wrkslots-eden-remove-resume-and-peer-unmount: resume Eden remove past a not-mounted checkout and unmount the whole peer group'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T15:30:00-05:00
updated_at: 2026-11-17T15:30:00-05:00
---

# Description

[watermelon-full-preview] (i) Resume a remove when the daemon lists the checkout "(not mounted)" after a recorded attempt: skip the `sl` status/revision precheck that today fails with "repository ... not found" on a checkout Eden already removed. (ii) Before Eden's rmdir, unmount EVERY FUSE entry of the checkout's mount peer group, deepest first, in the HOST namespace (bind propagation made 8 entries in one peer group; Eden unmounted only its own and failed with EBUSY).

# Design

- Use the existing peer-aware unmount helper where possible.
- Deterministic tests with fake mountinfo and fake Eden output.

# Acceptance Criteria

- A not-mounted checkout resumes removal without the `sl` precheck; every peer-group FUSE entry is unmounted deepest-first in the host namespace before rmdir.

# Notes

## Resolution

Landed ac5d8d060aa332b43a97d816b9b8d2744d087a10 on github/main (mu-edenfix). A "(not mounted)" checkout skips the `sl` salvage precheck and goes straight to the verified removal sequence; every FUSE entry of the checkout's `shared:<id>` peer group under the managed roots is unmounted deepest-first on the host before `eden remove`, and any peer outside refuses by name with nothing unmounted. Full gate pass; combined-tree overlap verification green. No new persisted field.
