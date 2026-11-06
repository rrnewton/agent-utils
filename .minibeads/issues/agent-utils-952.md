---
title: 'revive-device-pin: revive journals must not pin st_dev across a reboot'
status: open
priority: 0
issue_type: bug
external_ref: gh-305
created_at: 2026-11-06T18:25:00.505015056+00:00
updated_at: 2026-11-06T18:54:31.382742241+00:00
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
