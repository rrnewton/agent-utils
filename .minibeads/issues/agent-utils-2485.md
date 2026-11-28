---
title: 'image-unmount-long-path-edition-mismatch: for an unmounted image under a long root, the Rust edition fails with errno 36 where the Python edition refuses the arguments'
status: open
priority: 2
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- rust
- editions
created_at: 2026-11-28T13:08:00-05:00
updated_at: 2026-11-28T13:08:00-05:00
---

# Description

`py/wrkslots/tests/test_image_mount_family_rust.py::test_image_unmount_unmounted_matches_python` compares `wrkslots image unmount slot-a` between the editions, for a slot whose image is not mounted.

In one gate (head f775efd9, `python.wrkslots.shard-6`, 2026-11-27) the editions disagreed:
- Rust exited 1 with an error ending in "... error 36)". Errno 36 is ENAMETOOLONG.
- Python exited 3 with a REFUSED message about its arguments.

The temp root of that run was a long private directory. The Rust edition seems to reach a filesystem call whose path is too long before the check that Python refuses on, or it turns that call's failure into a plain error instead of the same refusal.

One occurrence in the kept logs: the console gate log in that lane's scratch directory. The full stderr was truncated there.

# Design

- Reproduce with a long temp root (a `TMPDIR` whose path makes the slot's image path exceed the kernel's name limits) and capture both editions' full output.
- Make both editions give the same refusal for the same condition, in the same order of checks. A path the kernel refuses as too long is a named refusal in both, not an unhandled error.
- A differential test with a long root, added to the same file.
- No wall-clock budget; no new persisted field.

# Acceptance Criteria

- Under a long root, both editions return the same exit code and the same refusal for unmounting an unmounted image.
- The new long-root differential passes in both editions, in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the 48-hour triage of `python.wrkslots.shard-6` reds.

Related:
- #2065: the wrkslots shards where it was seen.
