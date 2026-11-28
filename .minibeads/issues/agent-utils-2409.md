---
title: 'gate-red-step-logs-kept: keep the failing steps logs of every red gate row outside the 20-directory pruning, bounded by size and age'
status: closed
priority: 2
issue_type: task
assignee: cl-redlogs
labels:
- gate
- validation
- flake-triage
created_at: 2026-11-27T17:46:00-05:00
updated_at: 2026-11-28T13:03:00-05:00
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

## Resolution

Fixed in 9b68a3c1 (`scripts/gate.py`, new test file `py/tests/test_gate_evidence.py`, one `validation/components.json` entry). Gated on 4eca6aa1 as 1795886207981875601-89008a77, with exclusive rerun 1795888302697412454-54ea5753.
- When a gate or rerun has failed or NO_RESULT steps, gate.py copies those steps' logs, their journal records and a `manifest.json` to `<evidence root>/runs/<run-id>/`. It does this in-process, after the admission slot is released and before the gate-log prune. The row's `evidence_path` names the entry; it is null on every other row and the only new ledger field.
- Acceptance, each covered by a test that runs no real gate:
  - Logs survive 25 later gates: after 25 later runs the red run's own log directory is pruned (#2078 keeps 20) and its evidence is intact; one more end-to-end gate leaves it alone too.
  - Bounded: entries past 30 days are removed; the oldest go until the root fits 1 GiB; repeated red runs, some with 40 red steps, never take the root past its bound; each log is capped at 2 MiB (head and tail, the cut marked).
  - A copy failure leaves the verdict unchanged: with an unusable evidence root a red gate still exits 3 with verdict fail, and a green one still passes; the failure is printed and `evidence_path` is null.
  - The ledger field: gate and `--rerun` rows name their entry, with failed and NO_RESULT logs kept and a passing step's log not; `gate.py stats` reads rows with and without the field.
- First real entry: the feature's own gate (run 1795886207981875601-89008a77, red on `python.wrkslots.shard-13` and `rust.merge-queue.test`) kept both logs whole and 1372 journal lines out of 4.55 MB, in 0.03 s with 0 errors. That kept log named the wrkslots failure (`test_census_late_fork.py::test_naming_matches_read_argv_and_environ_from_proc`, #2065) without a local rerun.
- Cost: about 1 s for 64 MB of red logs plus a 17 MB journal, measured at host load 195, after admission release.
- Off-path reds in the gate: `python.wrkslots.shard-13` passed on the solo rerun; `rust.merge-queue.test` (#2458 CPU-TIMEOUT) was waived by the liaison. Landed with the coordinator's overlap exception: main had added separate list entries to `validation/components.json`; the combined tree's manifest load, validation-graph contract, these tests and strict mypy were green.
