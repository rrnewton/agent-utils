---
title: 'wrkslots-root-owned-artefacts: removal handles root-owned build files inside a slot'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T22:24:35.082939149+00:00
updated_at: 2026-11-05T22:24:35.082939149+00:00
closed_at: 2026-11-06T19:18:43.000000000+00:00
---

# Description

Root-owned build artefacts inside a slot block its removal. On 2026-11-05 a consumer team's slot held 30 root-owned files (133 MB under a Rust `target/debug/incremental/` directory), left by builds run in the root-isolation box. A non-root remove cannot delete them, so reclaim stalls.

# Design

Choose one, and document it:
- (a) wrkslots removal and cleanup delete root-owned files inside the slot with a scoped `sudo -n` command. It is limited to paths under the slot's own directory after resolving symlinks, refuses anything outside, and logs each path. wrkslots already uses `sudo -n` for mounts.
- (b) The root-isolation box keeps build output user-owned (for example, running the build step as the user, or chowning on exit), so no root-owned files appear.

(b) prevents the problem; (a) handles slots that already have such files. Both may be needed.

# Acceptance Criteria

- Test: a slot containing root-owned files (made via sudo in the test, or a fake privileged helper) is removed completely, and no path outside the slot is touched.
- Test: a symlink inside the slot pointing outside it is never followed by the privileged delete.
- With (b): a box build leaves no root-owned files in the slot.

# Notes

Reported by the lead on 2026-11-05 at 5:24 PM ET, from a consumer team's manual reclaim. Assigned to the dedicated wrkslots lane.

## Resolution

Landed in the mu-wrkslots stack at 84585684. Design (a): before Git removes the checkouts of a fenced slot, entries not owned by the invoking user are deleted through one `sudo -n rm -rf` scoped to the slot, parents resolved inside it, symlinks never followed, each path logged; documented in the user guide; tested with a fake privileged helper.
