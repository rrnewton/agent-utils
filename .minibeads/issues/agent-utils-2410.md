---
title: 'one-page-complete-browser frequent reds are era-bound (all 2026-11-20/21) with no kept failure log: name the signature before any known-red entry'
status: open
priority: 3
issue_type: task
labels:
- vibe-talk
- flake-triage
- validation
created_at: 2026-11-27T17:46:00-05:00
updated_at: 2026-11-27T17:46:00-05:00
---

# Description

`vibe-talk.check.one-page-complete-browser` was red on 31 distinct heads in the audit window (REAL 0, FLAKE 9, UNKNOWN 22).

- All 31 reds fall on 2026-11-20 and 2026-11-21, the #2077 hot-host episode.
- No step log survives, and no issue records a signature.
- No issue names this step's reds.

# Design

- Track the step. Make no known-red entry until a kept log (#2409) names the failure.
- If it reds again, read the kept log, name the wait or assertion, and harden it with a condition wait and a generous backstop (owner rule 346). Disable it with this issue as the tracker only if hardening cannot be done.

# Acceptance Criteria

- Either a named signature with a fix, or 14 days of gates without a red on an untouched head, after which this closes.

# Notes

Filed 2026-11-27 at the lead coordinator's request.
Source: the harness repository's `ai_docs/reports/measurements/2026-11-27-frequent-red-steps-audit.md`, item P1 and step 4.
Depends on #2409 for kept logs.
