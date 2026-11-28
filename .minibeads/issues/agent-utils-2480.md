---
title: 'local-ci-hub-s2-predicate-completion: one full-validation predicate, targeted-only answers, qualified-rows view'
status: open
priority: 1
issue_type: task
assignee: mu-rustport
labels:
- local-ci-hub
- rust-primary
created_at: 2026-11-28T12:02:00-05:00
updated_at: 2026-11-28T12:02:00-05:00
---

# Description

Slice S2 of the #1365 parity plan. The Rust qualifying predicate (`rs/local-ci-hub/src/verdict.rs`) is complete in its core but diverges from the authority in documented places: it skips the admission, disjoint-main and cell-results clause families, and its full-selection clauses require `profile == "full"` and `selection_mode == "full"` together. That strict AND is fail-closed (a targeted pass never qualifies — correct) but wrong in both directions on real records a consumer produces: new full runs carry `selection_mode: "full"` with no `profile` key at all, and large label-selected runs carry `selection_mode: "label"`.

The consumer has since defined ONE full-validation predicate as the single seat of this rule. This slice mirrors it exactly as one shared `full_validation` function used by every reader, closes the skipped predicate clauses against the authority's semantics, gives targeted runs their own named answer instead of a bare not-qualified, and adds the qualified-rows verdict view. Sequencing inside the slice: the mirrored predicate and its fixtures land FIRST, because the time-green metrics (#2469) build on them; the targeted-only answer and qualified-rows follow.

# Design

- One shared full-validation classification mirroring the authority's landed predicate: only `selection_mode` decides (`profile` is never consulted by this function). `full` counts. `label` counts only when the full node set at the run's exact target commit — the nodes carrying the configured full label in the configured DAG file at that commit, read from git objects — is non-empty and is a subset of the run's planned nodes (the union of started and skipped steps in its journal). Everything else (`only`, unknown or missing selection, receipt-shaped records) never counts. Missing journal, unreadable DAG, or empty full set fail closed. A covering label run counts with its whole verdict.
- The qualifying predicate's selection clause and the clean-full test both route through this one function — no strictness knob, no second spelling. Whether the receipt path keeps an additional profile clause on restored rows (the authority's receipt predicate has one; its run records may not carry the key) is decided with the fixture set during this slice and recorded in the issue, not decided silently in code.
- Skipped clause families (admission, disjoint-main admission proof, cell-results) implemented against the authority's semantics, with the stricter-list machinery: every remaining stricter verdict is listed by id and class, none waived by a standing allowance.
- Targeted-only answers: a commit whose best evidence is a targeted (`only`-family) pass answers as its own named kind, never PASS and never a plain not-qualified, in `receipt show|verify` and `ledger verdicts` alike.
- `ledger qualified-rows`: one verdict object per distinct row commit in the qualified-rows shape, byte-comparable against the authority's view on fixtures.
- Fixtures port the authority predicate's test cases in generic form (state integration is #2469's; here: the predicate truth table — full without profile, profile-full without selection, only-family, unknown/missing selection, label cover/non-cover/missing-journal/missing-DAG — plus the three record shapes as end-to-end receipt fixtures). Consumer-bound cases stay downstream evidence; nothing internal (hosts, paths, ids, codenames) enters fixtures.

# Acceptance Criteria

- `full_validation` exists once, is the only spelling of the rule in the crate, and its truth table matches the authority's on the ported fixtures (full-without-profile counts; label counts exactly on full-node cover; only/unknown/missing never counts; missing evidence fails closed).
- A targeted pass never yields a qualified verdict on any surface, and yields the named targeted-only answer where it is the best evidence.
- New-format full rows (selection full, no profile key) and covering label rows qualify end-to-end through `receipt verify` on fixtures; pre-existing digest and verdict fixtures are byte-unchanged except where a listed stricter case is intentionally resolved.
- Skipped clauses implemented or each remaining skip named in the module documentation with its diverge direction (looser/stricter) and a stricter-list entry; 0 looser than the authority on the fixture corpus.
- `ledger qualified-rows` output matches the authority's verdict view byte-for-byte on synthetic fixtures.
- Focused crate tests, `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` pass; the gate's selected checks pass.

# Notes

Slice S2 of the #1365 campaign; number pre-assigned by the coordinator (2026-11-28). The predicate portion is sequenced first inside the slice because #2469 (time-green form A) depends on it.
