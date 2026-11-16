---
title: 'wrkslots-read-scan-event-retention: read-only scans retain the whole parsed event log plus replay copies'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-16T23:10:00.000000000+00:00
updated_at: 2026-11-16T23:10:00.000000000+00:00
---

# Description

Follow-up to #1749. Measured on an ~85,000-event registry (raw event
bytes 1,084 MiB, about 13 KB per event): one `_load_events` raises
peak RSS to 2.24 GiB, and `_validate_global_state` on top of it to
4.63 GiB. A full audit peaks at 4.96 GiB address space. The parsed
log is retained whole per machine (`event_logs` for the scan's life),
and state validation folds the same log into replay state and parsed
records beside it, so the peak holds the log and its derivatives at
once. Per-event cost is about 55 KB of RSS against 13 KB on disk.

# Design

- Reduce retained copies: a scan should fold the log once (streaming
  with `_iter_events` where a fold suffices) and retain only derived
  state, not the parsed log plus its replay.
- Keep the memo's correctness rules (identity-bound reuse, retention
  caps for read-only scans) and every verdict identical.
- Re-measure peak address space/RSS on a large synthetic log before
  and after; keep deterministic count-based tests, no wall-clock
  budgets.

# Acceptance Criteria

- Peak RSS of a full audit on a synthetic ~85k-event registry drops
  materially (target: no whole-log parsed retention beside replay
  state), with identical audit JSON (modulo measured fields).
- Existing read-scan and audit tests pass unchanged in both editions
  where they implement the behaviour.

# Notes

Finding recorded while fixing #1749; the 10 GiB default bound there
is sized for today's retention, so this issue is what lets it come
back down.
