---
title: 'test-temp-and-mount-leaks: pytest shard basetemps and failed image tests leak temp trees and loop mounts'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-08T14:25:00.000000000+00:00
updated_at: 2026-11-08T14:25:00.000000000+00:00
---

# Description

Two test-hygiene leaks found by disk passes on 2026-11-08:

1. `scripts/run_pytest_shard.py` creates a `mkdtemp` basetemp (`/tmp/agent-utils-pytest-<pid>-<n>-*`) and never removes it. One host had 347 such directories (22 GiB, 2026-10-30 to 11-07; since removed); another has 88 (1.1 GiB, 10-29 to 11-05).
2. wrkslots image tests leak loop-mounted ext4 images when they fail. One host showed loop mounts under `/tmp` and `/var/tmp` `pytest-of-<user>/garbage-*/test_dbg*/project/worktrees/slot-images/...`, and `losetup -l` listed about 11 loop devices whose backing files were deleted (e.g. `.../test_remove_checks_an_image_ba0/.../slot.img`). The other host has 2 such loop devices (`/tmp<random>/project/worktrees/slot-images/agent/slot01/{slot,state}.img (deleted)`). pytest's garbage rename cannot delete through a live mount, and a deleted backing file still holds its blocks.

Related: #1061 (a leaked chat run from a failed test held a slot's mount).

# Design

- `run_pytest_shard.py`: remove its basetemp when the shard ends; keep it only on failure AND when asked (an explicit flag or environment variable), and say where it kept it.
- Image tests: teardown that always unmounts and detaches what the test mounted (fixture finalizer / try-finally), on failure as well as success, logging each step.
- A sweep for dead test mounts and loop devices: only entries whose owning test process is dead, whose backing file is deleted or lies under a pytest basetemp, and whose mount point is under a pytest basetemp. Use `sudo -n umount` and `losetup -d` on each verified entry, never by pattern, and log each action with what proved it was a test leftover. Dry run by default.

# Acceptance Criteria

- A shard run leaves no `/tmp/agent-utils-pytest-*` directory behind on success, and keeps it on failure only when asked.
- A test that fails mid-way through an image mount leaves no mount and no attached loop device (a test forces the failure).
- The sweep, given a fixture with one live test mount and one dead one, detaches only the dead one, and its dry run lists it without acting.
- The existing leaked mounts, loop devices and basetemps are cleaned on both hosts with the sweep (verified, logged), and the freed space is reported.

# Notes

Filed by the devbig030 coordinator at the lead's request. Given to cl-start after #1061 chat-launch-outer-kill-leak (same family). The other host is reachable with `agentcloudctl node exec`.
