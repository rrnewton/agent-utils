---
title: 'rust-ci-hub-ledger: a Rust local-ci-hub ledger core with fan-out storage'
status: open
priority: 1
issue_type: task
created_at: 2026-11-12T12:47:10.000000000+00:00
updated_at: 2026-11-12T12:47:10.000000000+00:00
---

# Description

The Python local-ci-hub proved the interface in shadow mode: six objects
(`run`, `ledger`, `receipt`, `remote`, `health`, `land`), append-only
`validate-ledger/v1` events, receipt-bound verdicts, and refusal rules that
never treat absence as green. It is not a sufficient implementation base: every
query refolds all history with no index, its Python canonical digest can
diverge from the authoritative typed digest, and configured git publication is
not implemented end-to-end.

Build the Rust successor in `rs/local-ci-hub` while preserving the existing
CLI, configuration, and verdict specification. Storage uses a fan-out `Store`
contract with one designated read authority per deployment; git is the
least-common-denominator connector and can act as either record or publish
route depending on deployment. Merge-queue support is optional and reuses the
existing Validator/run-results contract. This is the single successor, not a
parallel implementation.

# Design

- Core crate `rs/local-ci-hub`: typed `Run`, append-only slim `Event`,
  dereferenced `Receipt`, derived `Verdict`, `MeasurementWindow`, and
  `RetentionHold`. Exactly one fold, qualifying predicate, and canonical
  digest implementation. Full result payloads are content-addressed evidence
  fetched by digest; absence, `no_result`, infra refusal, timeout, and
  incomplete can never manufacture green.
- Derived SQLite index: tails the read authority by cursor, maintains
  by-commit/run/host and per-shard suffix posting lists, and is keyed by
  object digest plus schema version — never source-code hashes. Deleting and
  rebuilding it must not change verdict semantics.
- Fan-out storage: writes are acked independently per connector and the
  spool is retained on read-authority failure. In-tree connectors are local
  file/SQLite and git; downstream record connectors stay out of tree behind a
  versioned subprocess/plugin connector protocol designed in M2 at the
  latest (M1 if cheap).
- Merge queue: build a Validator adapter against `rs/merge-queue` using the
  existing exit/refusal, selection, per-test results, and tool-identity
  contract. Do not duplicate queue planning, donor leases, land drivers, or
  queue state.
- Edition transition: Python remains default and shadow oracle until Rust
  matches CLI/config/verdict parity; flip Rust to primary after the shadow
  gates, then delete the Python core after one release without fallback.

# Acceptance Criteria

- M1 delivers typed events, one fold, one digest/predicate, the incremental
  SQLite index, and read-only `receipt show` over the git connector.
- On synthetic upstream fixtures plus a locally supplied pinned evaluation
  corpus that is never committed here: zero verdicts looser than both Python
  oracle folds; stricter verdicts only in the two documented classes; folded
  row counts and terminal fields byte-identical as sorted JSONL; delegated or
  core digests byte-identical; conflicting event IDs and evidence
  hash/length/host refusals match by class.
- Performance wall/CPU/RSS is measured and reported against the existing
  full-scan baseline, but no wall-clock threshold is used as a gating test.
  Any acceptance report containing downstream identifiers remains outside
  this repository.
- The subprocess/plugin `Store` protocol covers versioned append, read,
  head, and object fetch with structured independent acks, replay-safe
  idempotency, and refusal classes distinct from validation failures.
- The queue Validator adapter accepts hub runs unchanged and preserves
  could-not-run `job` versus `host` scope.
- Relevant path-selected validation passes at every milestone; the Python
  edition remains the default until the recorded parity and shadow gates pass.

# Notes

Phase 1 agent-utils position note: harness
`ai_docs/plans/2026-11-12-rust-ci-hub-ledger.md` (2026-11-12). Coordinate
queue contracts with #1362 agent-utils-merge-queue-dogfood. A consumer owns
its out-of-tree record connector, history backfill, migration, and deletion
programme; upstream receives generic code and synthetic fixtures only.
