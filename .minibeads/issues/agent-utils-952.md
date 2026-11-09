---
title: 'revive-device-pin: revive journals must not pin st_dev across a reboot'
status: closed
priority: 0
issue_type: bug
external_ref: gh-305
created_at: 2026-11-06T18:25:00.505015056+00:00
updated_at: 2026-11-09T13:34:16.158212738+00:00
closed_at: 2026-11-09T13:34:16.158212628+00:00
---

# Description

revive-device-pin: revive journals must not pin st_dev across a reboot

GitHub rrnewton/agent-utils#305. Revive journals compared (st_dev, inode)
for pinned agent directories. btrfs anonymous device numbers change across
a reboot/remount (46 -> 48 on devbig014, 2026-11-06), so every revive in a
registry with a pre-reboot journal was refused as "old revive generation
changed while reserving identities".

Fix: journal-boundary comparisons use inode only (same_journal_directory);
generation token plus record SHA-256/exact bytes still identify the
generation, and intra-run pinned checks keep device+inode. Includes the
newer abandonable() journal sites and a ready-SHA check in retire
preflight. Regression test: device drift proceeds, inode/SHA drift refuses.

# Notes

## Resolution

[watermelon-full-preview] Delivered on main. Rust edition: 9e8f198d (the
landed twin of the scratch commit 621f1337; identical patch-id) converts the
revive journal comparisons to inode-only via same_journal_directory, and
d9234603 covers the two abandonable() sites the original patch predates,
adds the ready-SHA check to retire preflight, and adds the regression test
revive_journal_survives_device_change_but_still_pins_inode_and_sha (device
drift revives, inode or SHA drift refuses). Python edition parity landed
with this closing commit: journal identities in py/agentctl/revive.py now
carry no device at journal boundaries (_identity/_identity_matches), fresh
intra-run identities keep device+inode, and the same three-case regression
test covers it in py/tests/test_agentctl_lifecycle.py. Verified on the
closing tree: the Rust test passes (1 passed) and the Python revive suite
passes (88 passed, including the 3 new cases; the device case failed before
the parity fix with "revive record directory generation changed").
Unverified limit: no live reboot with a pre-reboot journal has exercised the
Python edition path; boot restore runs the Rust engine.
