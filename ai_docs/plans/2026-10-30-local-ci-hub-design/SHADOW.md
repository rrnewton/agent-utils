# Phase 3 shadow comparison (read-only)

Method: local-ci-hub folded the real published ledger
(`ledger/consumer-a/<host>/*.jsonl`, 4,395 rows) with no writes to the
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

## Update: evidence dereference + 100-commit rerun

row-ref/v1 evidence is now dereferenced (hash, length, and
decompressor exit verified; referenced commit/run must match the
envelope; a refusal is sticky across later events). With
full-evidence strictness on (anchoring + executed tests mandatory):
last 100 main commits agree 100/100 with ci-hub qualified-rows.
Across all 2,672 distinct ledger commits: 2,483 agree, 189
disagree — 169 where local-ci-hub is stricter (safe direction,
mostly any-FAIL vetoes and missing full evidence) and 20 where it
is looser (remaining canonical-predicate details such as gate
completeness not yet ported; sample `ee303899`). Canonical digest
still differs from the Rust HistoryRow digest (typed field order
and defaults); the Rust digest remains the authority for the
origin project. No PR field exists in ledger rows, so "PR heads in
the ledger" resolved to all distinct ledger commits above.
