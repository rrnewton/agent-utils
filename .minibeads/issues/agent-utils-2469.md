---
title: 'local-ci-hub-time-green: main green hours and the green-time integral, as configuration-driven metrics'
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

Slice S3 of the #1365 parity plan, moved ahead of the writer and run-engine slices at the owner's direction: the OSS-usable binary (S1, landed) plus the time-green metrics is the milestone due around Dec 9-11. Form A (main green hours) lands first inside the slice; form B (the green-time integral) follows in the same slice. Both are pure read/compute over configured inputs and depend only on the shared full-validation predicate from #2480.

Both metrics exist today only in a consumer's Python implementation. This slice ports them behaviour-exactly into `rs/local-ci-hub`, with everything consumer-specific — repositories, validate kinds, branch, the cell-matrix step prefix, push-time source, authoritative workflow names, the run-store location — as configuration in `local-ci-hub.toml`, never compiled in. Unknown and no-result never count as green in either form; flaky is its own state, never folded into green or red. Neither metric is a gate input: both are reported measurements only.

# Design

- Form A, main green hours (commit-state rule): each first-parent main commit is a state beginning at its push time (push records when configured, else committer time, clamped to first-parent order). Each commit is classified from full validations of exactly that commit (#2480's predicate; a covering label run counts with its whole verdict): GREEN some full validation passed; MILD RED failed with a journal whose matrix steps all passed; RED a matrix failure, an incomplete matrix, or a missing journal; FLAKY both a pass and a fail; UNKNOWN unless the nearest classified ancestor and descendant agree (then it inherits). Precedence FLAKY > GREEN > RED > MILD RED. Extra classification rows (bisect) fill only unclassified commits and accept only GREEN/RED by exact commit id. The tip's state integrates over the window into green/red/mild-red/flaky/unknown hours plus the longest red stretch (red + mild-red) and the current streak; recomputed from full history every run; per validate kind against its own repository's first-parent main; exit 0 measured, 2 inputs unreadable (primary line); outputs a paste-ready block and a typed daily aggregate (the five hour buckets plus flaky rate), with per-repository daily records idempotent on rerun and a 7-day rollup.
- Form B, green-time integral: commit reigns from a local hosted-CI run store (latest attempt per commit and authoritative workflow; combined precedence red > gap > no-result > green; a cancelled run promotes to red only when a per-job store proves a job failed before cancellation). Headline = green wall-seconds divided by (green + red) wall-seconds; no-result and gap time are reported beside the figure (hours, excluded hours, coverage) and never enter the headline denominator; time past the store horizon is an explicit gap. Day/week trend buckets use the same formula; each computation appends one snapshot line to a JSONL log. No live hosted-CI calls; the store is a configured local input.
- Consumer quirks are reproduced exactly and named in a deviation table in the implementation notes (mild-red inferred from matrix counters only; secondary-line failures printed as no-result yet exit 0; typed-aggregate write failures swallowed). Tightening any of them is a later, separate, flagged change. The one authorized deviation is the full-validation selection itself: the mirrored #2480 predicate replaces the older OR form, and the S8 shadow quantifies the difference on the pinned corpus (downstream report; upstream keeps synthetic fixtures only).
- CLI: `metrics green-hours --from ... --to ...` and `metrics green-time [--trend day|week] [--append-log]` (final verb spelling settled in the slice against the existing CLI's conventions), `--json` everywhere, exit codes as above. Run-record, journal, push-record, classification and run-store inputs are configured paths/formats documented in the README with synthetic examples.

# Acceptance Criteria

- Form A golden fixtures (generic ports of the authority's tests: commit-state classification incl. flaky/inherit/disagreeing neighbours, window integration with streaks, push-time clamping and branch selection, mild-red journal split incl. missing journal → RED, predicate cover cases) byte-match expected hours/aggregates; form B reign fixtures pin the denominator rule, cancelled-run promotion, horizon gap, and trend buckets.
- Both metrics classify GREEN only through the #2480 predicate — one shared function, no second spelling in the metrics code.
- Daily records are byte-identical on rerun over the same inputs; a late validation reclassifies past windows on recompute.
- No wall-clock threshold is a gating test; timings are reported in test output only.
- Focused crate tests, `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` pass; the example configuration gains the metric sections and still loads verbatim; README documents both metrics' inputs, formulas and exit codes.

# Notes

Slice S3 of the #1365 campaign; number pre-assigned by the coordinator (2026-11-28). Depends on #2480's predicate portion landing first. The deviation quantification against the consumer's pinned corpus happens in the S8 shadow downstream; this issue's fixtures are synthetic and generic.
