---
title: 'watch-restore-epoch-legacy-report: watch alarms "no completed restore report" when the only report predates #1405 epoch keying'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-13T12:13:52+00:00
updated_at: 2026-11-13T12:13:52+00:00
---

# Description

[opus 5.5] Follow-up to #1405 restore-epoch-manager-restart. After a host's watch moved to a build with #1405, it reported "no completed restore report for this boot" on EVERY pass, although the team was fully up (`restore --dry-run`: every agent alive, would_complete true). The only report for this boot was written by a pre-#1405 binary, so it has no user-manager identity, and `RestoreEpoch::matches_report` rejects it against today's known manager. Minting a new report would need a forced restore, which sends im_back notices to every lane, so it's not a fix.

# Design

Either (preferably both):
- Count a legacy report (no manager identity) for the current boot as matching when the current user manager's start time is EARLIER than the report's finished_at, which proves the report was written under this manager.
- Otherwise, report it distinctly ("report predates epoch keying") rather than "no completed restore report", and don't alarm on it.

# Acceptance Criteria

- A test: a pre-#1405 report (no manager identity) finished after the current manager started; watch treats it as complete for this epoch.
- A test: the same report finished BEFORE the current manager started (so a manager restart happened since); watch reports a missing restore for the new epoch.
- No change for reports that carry a manager identity.

# Notes

Numbered from devbig030's interim block (1400-1469).

## Resolution

[watermelon-full-preview] Both halves landed: `RestoreEpoch::legacy_match` counts a pre-epoch report (no manager identity) for the current boot as this epoch's when the current user manager (new `Host::user_manager_started`, from `ActiveEnterTimestampMonotonic` placed by kernel boot time) had already started when the report finished, and the shared completed-report pick uses that proof for restore replay and watch alike — so no forced restore is needed to mint evidence. A report finished before the current manager started still reads as the earlier epoch's (watch reports the restore missing, as before); with no proof either way the watch finding says the report predates epoch keying and does not alarm. Reports carrying a manager identity are judged strictly as before. Tests (`completed_boot_report_proves_legacy_report_by_manager_start`) cover after/before/unprovable/strict; contained `boot_report` 3 passed, clippy/check/fmt clean. Rust-only: the Python edition implements neither watch nor report reading.
