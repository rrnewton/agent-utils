---
title: 'boot-restore-retry: boot restore units give up on exit 75 instead of retrying, with no journal output'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-06T20:38:38.786489729+00:00
updated_at: 2026-11-09T13:51:21.320012720+00:00
closed_at: 2026-11-09T13:51:21.320012579+00:00
---

# Description

Both boot-restore user units failed exit 75 at the 2026-11-06 12:27 ET boot and stayed failed, with no journal lines. agentctl restore exits 75 on any refusal; at that boot the pre-#305 binaries refused every revive with 'old revive generation changed' (st_dev pin), so the boot path gave up. Fix: generated agent-restore.service and the devbig014 hosts unit retry on-failure bounded (StartLimitBurst=5/1800s, RestartSec=60) and send output to the journal. Hands applies host files.

# Notes

## Resolution

[watermelon-full-preview] Delivered on main in two parts. Bounded retry
landed in 51346b0f: the generated agent-restore unit retries on-failure
(StartLimitBurst=5/1800s, RestartSec=60) and logs to the journal. This
closing commit adds SuccessExitStatus=74 to the generated unit (exit 74 is
complete-except-refused-lanes; retrying it was pointless), moves the
per-boot restore report's default from the shared XDG state directory to
REGISTRY/restore-reports/ (its own subdirectory, per the growing-collection
rule; two registries on one host previously overwrote each other's
same-boot report), reads the legacy location as a fallback so a same-boot
report from an older build still counts for the already-restored and watch
checks, writes a best-effort partial report when restore fails before its
final report (at boot the user journal may be unreadable, so this file is
the evidence), and corrects the restore help and user guide (new default,
exit 74, slot-manager lookup never consults PATH). Tests: default-location,
partial-report, dual-read precedence, and the unit rendering assertion.
Unverified limit: host application stays with hands - the generated-unit
diff was sent for hosts-file application plus daemon-reload; no live boot
has yet exercised the registry-local default.
