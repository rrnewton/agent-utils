---
title: 'donor-probe-output: a faulted donor discards the probe failure text'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-11T11:00:00.000000000+00:00
updated_at: 2026-11-11T11:00:00.000000000+00:00
---

# Description

Observability: a faulted donor deletes its probe output after each re-probe and logs only "probing". One donor has failed 233 re-probes since 11-10 10:18 AM ET (cause "operational-tool: immutable tool root disappeared"), and nobody can see the current failure text.

# Design

In `rs/merge-queue/src/donor.rs`: retain the last probe failure text after a failed re-probe, in a log line or a status field, instead of deleting the only visible cause with the probe output.

# Acceptance Criteria

- After a failed fault re-probe, the latest probe failure text remains visible in the donor log or donor status.
- Repeated failed re-probes replace the retained text with the newest failure rather than accumulating unbounded output.
- Existing probe pass/fault-clearing behaviour is unchanged.

# Notes

Reported 2026-11-11. Issue-only for now. Item 3 of the donor-jobs-cap findings relayed to this lane.
