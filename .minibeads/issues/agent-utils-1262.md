---
title: 'gate-local-preflight: a full gate queues only after a local pass'
status: closed
priority: 1
issue_type: task
assignee: mu-gatefast
created_at: 2026-11-10T22:50:00+00:00
updated_at: 2026-11-10T22:50:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 344 (2026-11-10): PR development should
make sure work passes tests before going into the merge queue. Part 3
of the gatefast lane: gate.py refuses to queue a full gate unless the
same tree passed the selected checks locally — a recorded local
`validate.py --base github/main` pass for that tree hash within 24
hours — or the caller passes `--no-local-preflight REASON`, which is
recorded in the ledger.

# Design

- validate.py records a successful real run under its working-tree
  hash (computed with a throwaway GIT_INDEX_FILE, so the record names
  the content, not the commit).
- gate.py checks for a fresh (24 h) record for the same tree before
  queueing a full gate. Reruns, nested gates, --selected, --list and
  --self-test runs are exempt.
- `--no-local-preflight REASON` is the explicit escape hatch; the
  reason is logged to the ledger.

# Acceptance Criteria

- Focused tests in py/tests/test_gate_admission.py cover refusal with
  no record, admission with a fresh record, refusal with a stale
  record, and the escape hatch.
- Full gate: touched steps pass; untouched reds follow owner reply 346.

# Notes

Filed provisionally as #1252; renumbered to #1262 at landing (main
moved, and the provisional renumber had also renamed a neighbouring
lane's issue file by mistake — that rename is reverted here).
