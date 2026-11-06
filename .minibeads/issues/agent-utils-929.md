---
title: 'merge-queue-unparseable-result-row: an unparseable results row is host-scope could-not-run, never an Err that stops the run'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T12:04:31.628439502+00:00
updated_at: 2026-11-06T12:04:31.628439502+00:00
---

# Description

A consuming queue: adapter wrote duration_ms negative overflow row; validate.rs Err stopped run, no verdict, same batch re-validated forever unbounded.

# Design

Any unparseable row (bad line, unknown outcome, invalid test id) makes validation could-not-run host scope, detail names file line number parse error ~300 bytes, parsed rows kept for diagnosis but no verdict, bounded by refusal retries/backoff and deferral escalation as #927.

# Acceptance Criteria

Bad row: could-not-run host scope, run does not stop, not ejected/struck. Same bad row every attempt parked after bound with notification. One good fail + one bad: could-not-run not red, good row in record. Unknown outcome and invalid id same.
