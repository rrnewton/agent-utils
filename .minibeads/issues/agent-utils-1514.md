---
title: 'local-ci-hub-legacy-count-rows: qualify legacy count-based pass rows by counts in the Rust predicate'
status: closed
priority: 0
issue_type: bug
created_at: 2026-11-14T06:32:41.910120613+00:00
updated_at: 2026-11-14T07:59:35.414010838+00:00
closed_at: 2026-11-14T07:59:35.414010598+00:00
---

# Description

The Rust qualifying predicate in rs/local-ci-hub (verdict.rs) is stricter than the Python oracle on one remaining class: legacy count-based rows whose result is pass, which carry no cell-results evidence and do not set the full-coverage flag. The oracle qualifies such rows by their counts: its cell-results verdict is a non-blocking grandfathered-unknown when the row schema is below the cell-evidence minimum and the evidence is absent. The Rust predicate instead holds these rows to the cell-evidence path, so commits whose only qualifying evidence is a legacy count row read as not-qualified (stricter than oracle). Measured on the consumer ledger: 401 rows behind 339 commits share this shape.

# Design

Add a clause named legacy to row_qualifies in rs/local-ci-hub/src/verdict.rs. It applies ONLY when the row result is pass AND the cell-results field is absent AND the full-coverage field is unset. When it applies, the row qualifies by the same count rules the oracle uses for these rows (typed executed/passed/filtered counts, failure cap, gate accounting as applicable), read from the oracle predicate so counts and fields match exactly. Rows carrying cell-results evidence keep the strict cell-evidence path unchanged; no other clause changes. Document the clause in the verdict.rs module documentation (documented-directions list) as an intended oracle parity rule. Synthetic fixtures only; no deployment names, identifying field spellings, hosts or paths in this repository.

# Acceptance Criteria

Synthetic tests: legacy-shape pass rows qualify by counts across several count shapes; negatives stay refused: (1) cell-results present but failing, (2) full-coverage explicitly false, (3) result other than pass, and a legacy-shape row whose counts fail. An end-to-end case runs through the real ledger verdicts CLI path. On real deployment data the 339 stricter commits are re-measured: report how many now match the oracle and confirm zero looser-than-oracle verdicts, with the exact command and binary SHA; run outputs deleted afterwards.

# Notes

## Resolution

[watermelon-full-preview] Added the `legacy` clause to `row_qualifies` in `rs/local-ci-hub/src/verdict.rs` (`legacy_applies`): it applies only when `result == "pass"` and `cell_results` is absent-or-null and `full_coverage` is absent-or-null. When it applies, the row is judged by the oracle's count rules (typed count shapes, `passed == executed`, failure cap, declared gate accounting at count-capable schemas, nonzero-execution proof; an old-schema row must carry both counts) and the coverage and base-evidence clauses are skipped for that row only; the green-class gate still applies. Rows carrying cell evidence or an explicit full-coverage claim keep the strict path unchanged. Documented in the module docs as an intended oracle parity rule. Synthetic unit tests cover several qualifying count shapes and the negatives (cell evidence present, `full_coverage = false`, non-pass result, failing counts, single-count old-schema row); `tests/verdicts_e2e.rs` runs the shape through the real `ledger verdicts` CLI. The pre-existing `absent_coverage_refused` test now pins the strict path with cell evidence carried, since a legacy row without coverage is judged by the clause.

Real-data verification (deployment flags from `rs/local-ci-hub/README.md`, run outputs deleted afterwards): `host-quiet` returned 0, then `agent-utils/bin/cpuset-alloc run --cores 24 -- nice -n 10 <release local-ci-hub> --repo <deployment repo> --git-ref b20ee231bd7b2727f0cb5daeb2fdcb38011eaa74 --evidence-root <deployment archive> --binding-tree-field <deployment field> --base-field <deployment base field> --base-field <deployment base-tree field> --fixture-producer <deployment fixture producer> --evidence-head-field <deployment head field> ledger verdicts`. Post-fix binary SHA-256 `4f89564b373445089f622397f5c8b7fe4343c7160a825195ba938d53c9181098` (pre-fix `8698b2c96add6427ace97dfb8043f9d327dbf9ca3816027cc94da9d76d393121`). At this pin the described class does not reproduce on folded rows: over 5,709 folded rows and 3,938 commits, pre-fix and post-fix verdicts are byte-identical (1,388 qualified, 0 flips, 0 gained, 0 lost), and every legacy-shaped row is decided by clauses the oracle and this predicate share (profile, passed-count equality, anchoring, or the no-count refusal), with none reaching a coverage/base refusal. The clause therefore matches the oracle on 0 of the 339 reported commits at this pin because none of their rows are decided by the clauses it changes; it introduces no looser-than-oracle verdict (zero newly qualified rows or commits; row-level looser versus the oracle is 0). The remaining reproducible stricter set at this pin is 29 commits latched by a genuine failure on record, which is the disposition family, outside this ruling. Schema-10 oracle qualification through the retained-evidence checker was not reproducible from folded rows (checker qualifies 0/1,857), so schema-10 commit parity is unverified by that route; the clause cannot affect schema-10 rows' evidence path (they carry cell evidence and stay strict).
