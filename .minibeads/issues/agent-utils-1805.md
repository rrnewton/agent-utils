---
title: 'wrkslots-eden-remove-resume-and-peer-unmount: resume Eden remove past a not-mounted checkout and unmount the whole peer group'
status: open
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
