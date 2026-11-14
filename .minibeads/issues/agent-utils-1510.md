---
title: 'local-ci-hub-nested-canonical-digest: canonicalize nested evidence in typed struct order'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-14T02:42:14.072565420+00:00
updated_at: 2026-11-14T04:47:01.000000000+00:00
closed_at: 2026-11-14T02:42:39.606452659+00:00
---

# Description

Consumer champion condition 2: 1 of 30 sampled digests genuinely diverges in nested canonicalization (coverage artifact plus gate attempts in one validate run record).

# Design

Canonicalize nested typed evidence in struct order at every depth in digest.rs; untyped shapes keep generic sorted form; synthetic unit + differential tests only.

# Acceptance Criteria

Synthetic nested records byte-equal to typed oracle form; untyped shapes keep sorted form; full 5709-digest comparison green on champion front door via sent SHA.

# Notes

## Resolution

[watermelon-full-preview] Root cause: the one genuine divergence is a schema-7 validate run record whose `cell_results` evidence the typed oracle serializes in struct declaration order at every depth (evidence, artifact `{path, sha256, row_count}`, cell identities `{lane, category, test, mode, backend}`, cells, verdicts, bindings), while the Rust canonicalizer emitted it as a generic sorted-key value. The record's coverage artifact sub-object is dropped by the typed coverage shape on both sides, and its gate attempt lists already matched in the sorted extension area.

Fix in `digest.rs`: `cell_results` now canonicalizes in typed struct order when the value carries the exact typed evidence shape (exact key sets and typed fields at every depth, including verdict variants, comparison specs, and evidence bindings); any other shape keeps the generic sorted-key form, exactly as the typed reader's retained/raw fallback serializes it.

Verification: the genuine record's digest is byte-equal to the typed oracle locally (sha256 66bde1eb…); a synthetic differential against the typed oracle (real-shape record, shuffled written order, binding+diverged verdict, untyped fallback, unknown verdict state, nested extras with big integers/floats/unicode, schema-8 path shape, raw test-results, rich coverage) is all byte-equal; committed synthetic unit tests mirror the shape. The full 5709-digest front-door comparison remains the champion's to run on the sent SHA.

Correction (2026-11-14): the verification above exercised the digest library directly; it never ran the shipped `ledger digest-rows` path, and the `66bde1eb…` value was recorded without the command or binary that produced it. When the champion later reported the front door still printing the generic-form digest for the known divergent record at `5988536c`, a full front-door run settled it: at `5988536c` with the deployment head field applied, `ledger digest-rows` already prints the typed-oracle digest for that record (`22c2459c…`), and computing the receipt digest of the folded row directly gives the same value, while the reported value (`058491ff…`) is exactly what the same row produces under the default head field — the generic sorted fallback, because the genuine evidence carries its head under the deployment's own field name. The "not fixed" report therefore came from a run without the deployment head field, not from the CLI bypassing the typed canonicalizer. To pin the shipped path, `tests/digest_rows_e2e.rs` now runs the real binary over a temporary ledger (typed cell evidence, coverage sub-object, gates with attempts): with the deployment field the CLI prints the typed digest, without it the CLI prints the generic fallback and warns once that the configured head field was absent from every typed evidence record. `digest-rows` gained that warning; no digest bytes changed.
