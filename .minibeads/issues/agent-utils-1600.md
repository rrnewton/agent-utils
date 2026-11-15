---
title: 'imagehelper-root-ops-docs: document each privileged imagehelper operation and the nocow disk cost'
status: open
priority: 2
issue_type: task
external_ref: gh-291
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

Operators asked to grant passwordless sudo for the kernel image backend cannot see, in user-facing docs, exactly which operations imagehelper.py performs as root: the guide says kernel mode runs a sudo mount, while the code actually runs the helper through sudo to attach a loop device, mount, unmount, trim, grow, plus a root fallback that removes lost+found. Nor do the docs state that images are created with chattr +C (no copy-on-write), which also turns off filesystem compression: measured in the report, build output took 419 MB inside the image versus 119 MB compressed (about 3.5x).

# Design

Add a user-guide section enumerating each helper operation (mount/umount/trim/grow/rmdir-lost-found), its exact privileged actions and its path/descriptor checks, written for a sudoers audit, ideally with a narrow sudoers line granting only the helper. State the NOCOW requirement (create order in py/wrkslots/worktreesimage.py) and its measured disk cost next to the existing sparse-image sizing text, and consider whether images may keep compression where the host filesystem allows it.

# Acceptance Criteria

- The guide lists every operation the helper's usage string accepts, with no operation undocumented (cross-checked against py/wrkslots/imagehelper.py).
- The nocow cost figure is stated with its source measurement.
- The literal-config/help checks that gate the guide stay green.

# Notes

[watermelon-full-preview] Filed from GitHub #291 imagehelper-root-ops-docs (https://github.com/rrnewton/agent-utils/issues/291) during Request 098 triage. Docs-only.
