---
title: 'gate-red-step-logs-kept: keep the failing steps logs of every red gate row outside the 20-directory pruning, bounded by size and age'
status: open
priority: 2
issue_type: task
assignee: cl-redlogs
labels:
- gate
- validation
- flake-triage
created_at: 2026-11-27T17:46:00-05:00
updated_at: 2026-11-28T12:05:00-05:00
---

# Description

The frequent-red steps audit of 2026-11-27 (P1) found that the gate keeps per-step logs (#2078) only for the newest 20 gate runs.

- Of 567 audited red head-occurrences, exactly one still had its step log on disk.
- Every other flake signature could be named only from an issue, or not at all. One example: all 31 `vibe-talk.check.one-page-complete-browser` reds (see #2410).
- A red row whose log is pruned cannot be triaged, cannot back a known-red entry, and cannot tell a real failure from a load failure.

# Design

- When a gate or rerun row is red (any failed, incomplete or NO_RESULT step), gate.py copies those steps' logs into `~/.local/state/agent-utils/gate-evidence/<run-id>/`, together with their test-boundary journal lines. That directory sits outside the 20-directory log pruning.
- Each copied log is capped at about 2 MB: its head and tail, with the cut marked.
- The evidence root is bounded: entries older than 30 days go first, then the oldest entries until the root is under about 1 GB.
- The ledger row gets `evidence_path`.
- Best effort: a failed copy is reported and never changes a verdict.
- The row-writing code is shared with #2401 (passed_steps in rows), so coordinate with its owner.

# Acceptance Criteria

- A red row's failing step logs survive 25 later gates, and the row names them.
- The evidence root never exceeds its size bound, and entries past 30 days are removed.
- A copy failure leaves the gate's verdict and exit status unchanged.
- Tests cover the cap, the bounds and the ledger field without a real gate.

# Notes

Filed 2026-11-27 at the lead coordinator's request (number allocated in one batch with #2410-#2415).
Source: `ai_docs/reports/measurements/2026-11-27-frequent-red-steps-audit.md` in the harness repository, item P1.
Today's 22 kept log directories total 64 MB, at most about 8 MB each, so the copies are small.

Implementation notes (cl-redlogs, 2026-11-28):
- Entries live in `<evidence root>/runs/<run-id>/`, not directly in the root, so the root's lock file never sits among the growing collection (the repository's directory rule). The root is `$GATE_EVIDENCE_ROOT`, else `gate-evidence` beside the gate-log root.
- Each entry holds the red steps' capped logs, `journal.jsonl` (the records of those steps, test boundaries included, plus run-level records other than containment) and `manifest.json` (per step: failed or incomplete, source and kept bytes, cut or not, why a log was missing).
- `evidence_path` is the only new ledger field: a path on red rows whose evidence was kept, null on every other row; readers must also tolerate its absence in older rows.
