---
title: 'restore-epoch-manager-restart: a systemd user-manager restart without a reboot replays the old boot report and revives nothing'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-11T21:24:28+00:00
updated_at: 2026-11-11T21:24:28+00:00
---

# Description

[opus 5.5] On 2026-11-11 an agent sent SIGTERM to a PID it took for a stale script's parent; it was the host's `systemd --user` manager. The manager went to exit.target at 11:30:41 PT, stopping every user unit (Herdr, the coordinators and their lanes, the bridges and the queue units), and a new manager started at 11:38:40. `agent-restore.service` exited 74 at 11:38:41, one second after the new manager started, and revived nothing. Every lane had to be revived by hand.

Cause (partial evidence from the host's restore agent, which itself died on rate limits before it collected the full report): "the real run replayed a Nov 9 boot report instead of restoring; boot_id never changed". `agentctl restore` keys its replay guard on the kernel boot id: "A completed restore for this boot makes a rerun replay the recorded report (action already-restored, replayed true) and exit by that recorded class, unless --force." A user-manager restart isn't a reboot, so the boot id stayed the same, and the boot unit replayed the Nov 9 report (class 74: completed except for refused lanes) instead of restoring.

# Design

- Key the restore epoch on something that changes when every user unit has been stopped, not only on the kernel boot id. For example, the user manager's own start (the invocation id or start time of `user@UID.service`, or the manager's PID and start time), combined with the boot id.
- When the epoch changes and the recorded report belongs to an earlier epoch, run a real restore; replay only when the epoch matches.
- The replay guard still protects against a second boot-unit run or a retry within one epoch. That's why `--force` must not become a default in the shared restore wrapper.
- The report records the epoch (boot id plus the manager identity) so a replay names which epoch it replayed.
- restore and restore-teams behave the same way; Python parity if the Python edition has the guard.

# Acceptance Criteria

- A test with a fake host: same boot id, new user-manager identity, and a completed report from the earlier epoch. restore runs for real (not replayed) and revives the running-at-shutdown lanes.
- The same boot id and the same manager identity still replay (a retry doesn't launch duplicates).
- The restore report shows the epoch and, on a replay, which earlier report it replayed.

# Notes

Evidence still to attach: this boot's report from the affected host's registry restore-reports/ (boot_id, path, replayed, action, refused_lanes, and the Nov 9 report it replayed). An opt-in `--force` passthrough in the shared restore wrapper is a separate, reviewed change; it must not become the default. Numbered from devbig030's interim block (1400-1469).
