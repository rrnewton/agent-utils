---
title: 'watch-restore-report-path: agentctl watch reports no restore report when one exists'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- restore
- watch
created_at: 2026-11-20T14:43:46-05:00
updated_at: 2026-11-20T14:43:46-05:00
---

# Description

[watermelon-full-preview] From a consumer team's 2026-11-20 reboot drill (relayed by the lead coordinator): `agentctl watch` reported no restore report for the boot although a report existed. As relayed, watch looked for reports at `.agentctl/restore-reports/` while restore writes `.agentctl/restore/reports/`. First checks against the pinned snapshot and current code complicate that: `candidate_report_paths` (rs/agentctl/src/restore.rs) already searches the current location (`REGISTRY/restore/reports/`, #1359) first, then the legacy ones, in every build inspected. The same symptom has a second candidate mechanism: `boot_report_state` skips any report with `complete != true` and then reports `Missing`, so a boot whose restore ran and wrote an incomplete report is indistinguishable in the watch finding ("no completed restore report for this boot") from a boot with no report at all — exactly the drill's situation (their report exists, `complete: false`, coordinator refused). Reproduce first and fix whichever mechanism (or both) is real.

# Design

Design note approved in shape by the lead coordinator (2026-11-21): acknowledgement first, because it avoids running restore at all for a boot whose lanes were brought back by hand.

(0) Reproduce on a scratch registry with a fake Herdr: (a) a complete report only at `restore/reports/`; (b) an incomplete report only at `restore/reports/`; (c) a complete report only at the legacy `restore-reports/`; (d) an explicit `report_dir` config. Record the watch `restore_check` finding for each on current main and on the consumer's pinned snapshot builds. (First checks already show `candidate_report_paths` searches the current location first on both, so the incomplete-collapse below is the prime suspect; the reproduction decides.)
(1) Acknowledge-incomplete (primary path): `agentctl restore --acknowledge-incomplete [--reason TEXT]` refuses unless a report for the current boot+epoch exists and is incomplete, and refuses if already acknowledged. It appends one acknowledgement object into that report file atomically (the existing report write path): acknowledged_at, acknowledged_by (the identity agentctl already records for queue operations, never free text alone), optional bounded reason, boot_id, epoch. It starts nothing, revives nothing, and touches no agent record.
(2) Report states: `BootReportState` gains `Incomplete(path, detail)` and `Acknowledged(path, ack)`; `Missing` keeps meaning no report file exists in any candidate location. Watch findings: Acknowledged → ok, naming who/when. Incomplete with EVERY desired lane verified live (the same desired-set derivation and live evidence a dry run's would_complete uses) → one info finding naming that state and the acknowledge remedy, then suppressed for that boot+epoch (watch alert dedup). Incomplete with any desired lane not live → alarm naming the missing lanes and the report's coordinator refusal reason, never the bare "no completed restore report" text.
(3) Path hygiene: every watch report read goes through `candidate_report_paths` (one reader, current location first, legacy fallbacks until 2027-02-12) — pin it whatever the reproduction shows.

# Acceptance Criteria

- The four reproduction registries above each produce the correct finding: complete-at-current and complete-at-legacy count as restored; incomplete-at-current names the existing report path and its incompleteness reason; explicit `report_dir` is honoured with no fallback reads.
- `--acknowledge-incomplete` on an incomplete current-boot report records {acknowledged_at, acknowledged_by, reason, boot_id, epoch} and a second run reports the existing acknowledgement without changing it; on a complete or missing report it refuses and writes nothing; it starts no agent and no unit (fake-Herdr command log empty).
- Watch on an acknowledged boot is ok and names the acknowledgement; on an unacknowledged all-lanes-live incomplete boot it emits the info finding once and then suppresses it for that boot+epoch; with a desired lane down it alarms naming the lane and the coordinator refusal from the report.
- No watch code path constructs a report path outside `candidate_report_paths` (pinned by review of the diff; existing boot_report_state tests extended, not weakened).
- The pinned older snapshot's behaviour is characterized in the commit body (which mechanism it had), so a consumer on that snapshot knows whether an upgrade alone fixes their finding.
- Existing restore/watch tests stay green; the only persisted addition is the acknowledgement object inside the existing report JSON (additive key; a test pins that an older reader still classifies the report by `complete` alone).

# Notes

Related: #1975 restore-boot-recovery ((2b) report-on-every-exit makes incomplete reports more common and more useful; this issue makes watch able to see them), #1359 (reports moved under `restore/reports/`), #1405/#1426 (epoch keying and the predates-keying watch wording). Number pre-assigned by the lead coordinator. Rides the Phase 2 stack unless the reproduction shows a one-line lookup fix that can land independently.
