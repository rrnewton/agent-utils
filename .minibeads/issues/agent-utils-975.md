---
title: 'boot-restore-retry: boot restore units give up on exit 75 instead of retrying, with no journal output'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T20:38:38.786489729+00:00
updated_at: 2026-11-06T20:40:19.273525865+00:00
---

# Description

Both boot-restore user units failed exit 75 at the 2026-11-06 12:27 ET boot and stayed failed, with no journal lines. agentctl restore exits 75 on any refusal; at that boot the pre-#305 binaries refused every revive with 'old revive generation changed' (st_dev pin), so the boot path gave up. Fix: generated agent-restore.service and the devbig014 hosts unit retry on-failure bounded (StartLimitBurst=5/1800s, RestartSec=60) and send output to the journal. Hands applies host files.
