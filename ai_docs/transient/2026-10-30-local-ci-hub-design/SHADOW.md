# Phase 3 shadow comparison (read-only)

Method: local-ci-hub folded the real published ledger
(`ledger/hermit/<host>/*.jsonl`, 4,395 rows) with no writes to the
origin project, its ledger, or any lock. Verdicts for the last 30
main commits plus the three named commits were compared with
`ci-hub ledger qualified-rows` (1,372 qualified commits).

Result: 30/30 binary agreement (qualified vs not). Named commits:
`dc85ec16` qualified/qualified, `df10f4c2` not/not, `72fc0eda`
not/not (four FAIL rows). The full per-commit table is retained in
the lane scratch (`ignored/shadow-table.json`).

Latent strictness gap (no verdict disagreement, real phase-3
work): recent PASS rows carry `legacy_row_codec: row-ref/v1`, so
the full row (executed tests, anchoring, gates) lives in external
compressed evidence this reader does not yet dereference. Our
qualified verdicts for those commits rest on the event-level
outcome, while ci-hub verifies the referenced evidence. Before
any authority switch, local-ci-hub must resolve row-ref evidence
and refuse when it is missing or mismatched.

## Wrappers: thin wrapper or delete (draft, not applied)

Become thin wrappers over `local-ci-hub receipt|ledger`:
`bin/reconcile-receipts` (already a 7-line alias), `validate-status`
(-> `receipt show`), `local-history` (-> `ledger query`).
Delete after caller migration (aliases of CLI commands):
`bin/load-probe`, `bin/main-health`, `bin/pr-status`.
Stay project-owned (not CI evidence): verified git/gh wrappers,
task/chat/escalation scripts, landing sequence, health probes,
compatibility website, series store.
