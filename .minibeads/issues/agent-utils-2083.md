---
title: 'wrkslots-owner-retires-absent-row: a slot''s live owner can retire its own absent-directory row'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T18:35:00-05:00
updated_at: 2026-11-21T18:35:00-05:00
---

# Description

When a slot's directory is gone, its row stays in the registry, and its live owner cannot retire it; only other paths can, so consumer launchers strand rows after a failed start.

# Design

[watermelon-full-preview] `wrkslots remove SLOT --retire-absent-row --agent AGENT` (plus the ordinary `--coordinator-pid`/`--expected-generation`): under the mutation locks it rechecks that the slot directory is wholly absent (image-backed rows refuse as merely unmounted), no mutation journal for the slot is outstanding, and the recorded owner process is live (pid + start time + boot identity) with `--agent` naming the row's agent; then it deletes the ACTIVE row with the ordinary `slot-removed` active-state event whose evidence names the basis `owner-retired-absent-directory`. No coordinator attestation, no salvage/census/Git work. No new event kind or action; one new evidence key on an existing event (older readers replay evidence generically). The Rust remove writer does not grow the flag in this change and refuses it by name until wired.

# Acceptance Criteria

- The live owner retires its absent-directory row; others still cannot; tested: `test_retire_absent_row.py` covers the live-owner retire, a wrong-agent refusal, a dead-owner refusal, and a present-directory refusal, each with the row state checked afterwards.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.
