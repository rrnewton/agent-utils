---
title: 'wrkslotsd-census-enametoolong-parent-walk: the Rust slot-use census places a same-device link readlink cannot name by its parents, as Python does, instead of exiting 1'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- rust
- census
created_at: 2026-11-26T21:00:00-05:00
updated_at: 2026-11-26T21:00:00-05:00
---

# Description

The slot-use census that `wrkslots image unmount`, `remove` and other writers run before touching a slot reads every process's `/proc/PID/cwd`, `root`, `exe` and `fd/*` links. When `readlink(2)` on one of them fails with `ENAMETOOLONG` (the process holds a directory deeper than `PATH_MAX`) and the link's `stat` lands on the slot's own filesystem device, the two editions disagree:

- Python (`#618`, `_too_long_link_within` / `_directory_link_holder` in `py/wrkslots/cli.py`) places the directory by walking its parents: inside the slot is use, outside is foreign, and a failed walk refuses.
- Rust (`observe_link` / `observation_points_into` in `rs/wrkslotsd/src/remove_write.rs`) has no walk: the link stays undecided and the command exits 1 with `process use is indeterminate because /proc/PID/fd/N failed: File name too long (os error 36)`.

So while any process on the host holds a path deeper than `PATH_MAX` on the slot's device, every Rust-edition census on that device fails, where Python proceeds. Found from a red `python.wrkslots` shard (`test_image_mount_family_rust.py::test_image_unmount_unmounted_matches_python`: Rust exit 1 with error 36 against Python's exit 3 refusal) that overlapped `test_census_long_paths.py`, which holds such fds while it runs. Reproduced deterministically on 2026-11-26 with a non-root process holding a deep fd under the temporary directory: the differential fails every time; without it, it passes 5 of 5.

# Design

Port Python's rule so both editions give the same verdict:

- `ENAMETOOLONG` on a link whose `stat` lands on the slot's device: open the link's directory (through the `/proc` link itself) and walk `..` with `openat`, comparing each directory's `(st_dev, st_ino)` with the slot root's.
- Reached the slot root: the process uses the slot; refuse and name the holder, exactly as for a readable link inside the slot.
- Reached `/` without meeting it: foreign, no evidence.
- Any error during the walk: an indeterminate refusal, exit 3 with the `REFUSED:` text and the walk's error, never exit 1.

Other errnos keep today's handling (gone process: no evidence; `EACCES`/`EPERM`: unreadable refusal; `EINVAL`: not a link). No persisted fields change.

# Acceptance Criteria

- A Rust unit test places a too-long directory inside the slot (use), outside it on the same device (foreign), and an unwalkable one (refusal, exit 3), with the deep fd held by the test process, never by a separate host process left running.
- The Python/Rust differential gains the same three cases and both editions agree on exit code, stdout and stderr.
- `test_image_unmount_unmounted_matches_python` passes while a process holds a `PATH_MAX`-deep directory on the same device.
- No root needed by any new test; no wall-clock budget added (owner rule 346).

# Notes

Found and reproduced by `cl-flaketriage` (coordinator's assignment, 2026-11-26). The 2026-10-28 incident `test_census_long_paths.py` cites (every validate-checkout removal on the host refused with "File name too long") is the same class on the Python side, fixed there by `#618`/`#651`; this ports it. Stacking: `remove_write.rs` is also changed by the queued wrkslots stack (`#2000`, `#2168`, and possibly later members); this change rebases onto whatever of it has landed before its gate.
