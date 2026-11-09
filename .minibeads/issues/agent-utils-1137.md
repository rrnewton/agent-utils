---
title: 'restore-waits-for-certificate: restore burns its one boot attempt launching agents before the user certificate exists'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T13:53:37.000000000+00:00
updated_at: 2026-11-09T13:53:37.000000000+00:00
---

# Description

[watermelon-full-preview] From the 2026-11-09 host reboot. Boot restore
launched three Claude lanes before the user TLS certificate existed (it was
renewed only later that morning). All three harness processes died
instantly, yet the restore report counted them as started and the boot read
as restored, so nothing retried them.

The #742 certificate gate exists (RestoreConfig certificate_file /
certificate_fetch, rs/agentctl/src/restore.rs) but is opt-in and
existence-only: with no certificate_file in restore.json the report says
"not configured" and restore proceeds; with one set, any regular file
passes, checked once with no wait. Separately, "started" is judged at first
detection: launch() flips the candidate to running as soon as Herdr reports
the harness and agentctl anchors it, restore fixes the row at "revived",
and nothing re-checks survival before the report computes complete. A
harness that presents its composer and then dies on first TLS use is
therefore revived forever for that boot, and a rerun early-returns
already-restored.

# Design

- Wait for the certificate, bounded: poll like wait_for_herdr under a
  certificate_wait_seconds budget (defaulting to the Herdr wait), and
  require a usable certificate (parseable PEM, not expired) rather than any
  regular file. Still missing at the deadline keeps today's refuse-all with
  the exact fetch remedy, so the boot unit's bounded retry can succeed once
  the certificate lands.
- Survival re-check: before computing complete, re-resolve every lane this
  run reported revived/promoted/revived-fresh; a lane whose runtime is gone
  is reported died-after-launch and makes the boot incomplete (the
  coordinator likewise), so the run stays retryable instead of reading as
  success.
- Document certificate_file / certificate_fetch wherever restore.json keys
  are listed. Host registries still name their own certificate path;
  agentctl does not hardcode one host's credential location.

# Acceptance Criteria

- Restore with a certificate_file that appears during the wait revives the
  lanes and the report shows the wait; one that never appears revives
  nothing, names the fetch remedy, and exits retryable.
- An expired or unparseable certificate file does not pass the gate.
- A lane whose harness dies right after launch is reported
  died-after-launch with complete false; a later rerun attempts it again
  instead of already-restored.
- Existing restore behavior without certificate_file is unchanged apart
  from the survival re-check.

# Notes

Mitigation already applied by hand on the reporting host: both registries'
restore.json now name their certificate_file, so the next boot refuses
(and the unit retries) instead of burning launches. This issue is the code
half: wait/validity plus not counting instant deaths as started. Related:
#742 restore-team-spec (the gate), #975 boot-restore-retry (the retry).
