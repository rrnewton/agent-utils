---
title: 'rust-ci-hub-ledger: a Rust local-ci-hub ledger core with fan-out storage'
status: open
priority: 1
issue_type: task
created_at: 2026-11-12T12:47:10.000000000+00:00
updated_at: 2026-11-12T15:05:00.000000000+00:00
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

- Core crate `rs/local-ci-hub`: typed `Run`, append-only slim `Event`
  (`run.start|run.result|run.enrich|run.correct|run.annotate`, with
  `run.annotate` as a metadata-only kind that never changes derived state and
  is refused when it carries outcome fields or a value-changing reference),
  dereferenced `Receipt`, derived `Verdict`, `MeasurementWindow`, and
  `RetentionHold`. Exactly one fold, qualifying predicate, and canonical
  digest implementation, computed in the core only; a delegated digest is
  allowed only as a parity-test oracle and no production path spawns a digest
  subprocess. The union collapses identical copies of an event and refuses
  the whole read when the same `event_id` carries a different body (no
  first-wins or last-wins). Full result payloads are content-addressed evidence
  fetched by digest; absence, `no_result`, infra refusal, timeout, and
  incomplete can never manufacture green.
- Derived SQLite index: versioned schema (`ci-hub-index/v1`), tails the read
  authority by cursor, maintains by-commit/run/host and per-shard suffix
  posting lists, and is keyed by object digest plus schema version — never
  source-code hashes. Opening an index at any other version discards and
  rebuilds it; deleting and rebuilding it must not change verdict semantics,
  proven by a rebuild-from-authority test (incremental equals cold rebuild).
- Fan-out storage: writes are acked independently per connector and the
  spool is retained on read-authority failure. In-tree connectors are local
  file/SQLite and git; downstream record connectors stay out of tree behind a
  versioned subprocess/plugin connector protocol: contract in M1,
  implementation in M2. Object fetch is bounded and streaming with a declared
  length and `max_bytes` cap, digest-verified at end of stream. The cursor is
  the authority head plus a per-shard offset and prefix digest; a squash or
  head rewrite invalidates it and forces a verified rescan, never a silent
  skip.
- Merge queue: build a Validator adapter against `rs/merge-queue` using the
  existing exit/refusal, selection, per-test results, and tool-identity
  contract. Do not duplicate queue planning, donor leases, land drivers, or
  queue state.
- Edition transition: Python remains default and shadow oracle until Rust
  matches CLI/config/verdict parity; flip Rust to primary after the shadow
  gates, then delete the Python core after one release without fallback.

# Acceptance Criteria

- M1 delivers typed events (including metadata-only `run.annotate`), one
  fold, one digest/predicate, the incremental versioned SQLite index with a
  rebuild-from-authority test (incremental equals cold rebuild), and read-only
  `receipt show` over the git connector.
- M1 also delivers the versioned wire contract `ci-hub-store-wire/v1`
  (document plus synthetic conformance transcripts) covering append, read,
  head, and bounded streaming object fetch with structured independent acks,
  replay-safe idempotency, refusal classes distinct from validation failures,
  and a squash-safe cursor with verified rescan; the contract is accepted in
  writing by a consumer before M1 exits. The subprocess/plugin implementation
  of that contract is M2.
- On synthetic upstream fixtures plus a locally supplied pinned evaluation
  corpus that is never committed here: zero verdicts looser than both Python
  oracle folds; stricter verdicts only in the two documented classes, each
  listed by commit id, and zero stricter before the shadow switch completes
  (M4); folded row counts and terminal fields byte-identical as sorted JSONL;
  the core digest byte-identical (a delegated digest allowed only as a test
  oracle; no production digest subprocess); the same `event_id` with a
  different body refuses the read; `run.annotate` metadata-only acceptance and
  its refusal class; and evidence hash/length/host refusals matching by class.
- M1b delivers series-corpus read parity (row counts plus digests, with
  conflicting-id refusals) through a generic streaming union reader for opaque
  rows, on a locally supplied pinned corpus that is never committed here.
- Performance wall/CPU/RSS is measured and reported against the existing
  full-scan baseline, but no wall-clock threshold is used as a gating test.
  Any acceptance report containing downstream identifiers remains outside
  this repository.
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
