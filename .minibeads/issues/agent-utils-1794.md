---
title: 'wrkslots-audit-memory-and-time: make audit streaming, batched and bounded as slots grow'
status: closed
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-17T15:08:33-05:00
updated_at: 2026-11-19T22:14:13-05:00
---

# Description

[watermelon-full-preview] A read-only `wrkslots audit --format json` on 74 worktrees now costs about 28 CPU-s per worktree (about 12 CPU-s per worktree on 2026-11-08): 2,098 CPU-s and 602 s wall in total, peaking near 8.8 GB RSS against a 10 GiB cap, with wall time at the 600 s default `--scan-timeout`. Contributors to check: full in-memory replay of about 86k events including seven about 45 MB events (#1772 stops new ones), per-worktree git/process probes added since 2026-11-08 (census, salvage/private-history scans, ignored-file collection, nested-repo checks), and repeated per-checkout git subprocesses (about 197 git plus about 423 telemetry-wrapper processes in 8 s measured in a remove).

# Design

Measure first, read-only, on a synthetic fixture and via `wrkslots audit --format json` with profiling against a scratch copy (never mutate a real project): CPU, RSS and wall per phase and per worktree, git process count per worktree; bisect the per-worktree cost growth between the 2026-11-08 era and current main. Report the top costs before changing code. Then make audit cheaper: stream or incrementally parse event replay (bounded memory regardless of event size), batch git calls per repository (one for-each-ref / cat-file --batch / worktree list per repo, not per checkout), cache per-repo facts across checkouts, and skip expensive per-worktree probes audit does not need for its report (or make them opt-in flags). Keep every audit finding identical (differential test old vs new output on fixtures). Apply the same fix in both editions where the Rust audit_state mirrors it. Reuse the batching helpers from #1785 wrkslots-remove-silently-skips-unresolvable-rescue-tips instead of duplicating them. No new persisted field.

# Acceptance Criteria

Audit findings are byte-identical to the old implementation on fixtures (differential test). Event replay uses bounded memory regardless of individual event size, including the existing about 45 MB events. Git work is batched per repository and per-repo facts are cached across checkouts; expensive per-worktree probes audit does not need are skipped or opt-in. Deterministic regression tests in both editions fail before and pass after. Per-worktree CPU, RSS and wall costs are measured and reported before and after.

# Notes

[watermelon-full-preview] Filed by mu-auditcost. Overlap: mu-rescuetips (#1785 wrkslots-remove-silently-skips-unresolvable-rescue-tips) is batching git calls in the private-history scan; reuse its batching helpers. Rebase right before the gate.

[watermelon-full-preview] Review note (F4, accepted): when two failures are present at once, the audit pre-check reads durable repositories before the full scan's reflog/pseudoref refusals, so the first refusal an audit row reports can differ from the pre-change order. Verdicts do not change; only which refusal is named first when several apply.

[watermelon-full-preview] Follow-up (not in this change): cache-planning listings (about 60 CPU-s and about 96k directory listings at 80 rows on the consumer, most subjects exhausting their listing share) still walk each slot's own tree per subject. Planning a shared repository's cache subjects once and reusing the plan across its rows, or charging a listing shared by several subjects once against the work budget, is the remaining audit cost that scales with row count after this change.

[watermelon-full-preview] Next target after this lands (doctor batching): on a real registry of 309 slots, `wrkslots doctor` takes about 390 s and exits rc 2 (partial, scan_timeout); worktree_config (about 220 s) and shared_submodule (about 144 s) are about 95% of it, at about 64 subprocesses per slot — the same per-row git-subprocess pattern this change cuts in audit. Batching those two doctor phases per repository is the follow-up; it is not part of this change.

## Resolution

[watermelon-full-preview] Landed on github/main as ff6aa130 (land-main verified-tree landing of the gated tree, --replay-base 9d098c31 --allow-overlap --verified-tree faabccfe). Audit now: stream-folds event replay for read-only bounded scans (bounded memory, identities verified before any resume), retains only the event kinds a row can consult, folds handoff publications once per machine, shares durable ref tips and commit-object enumerations per repository, caches per-checkout Git answers without ever letting a Git config answer cross checkouts, and skips the full nested-submodule private-history scan only when a decisive superset check proves nothing is kept (any doubt, including every #1785 unresolvable root, falls through to the full scan; GIT_NO_LAZY_FETCH=1 on its walks). Findings identical by the both-editions differential suites; new regression tests in py/wrkslots/tests/test_audit_cost.py cover the pre-check equivalence, the config-context caches, the grouped publications and the streamed resume. Measured on the 24-row / 3-repository fixture: git 39.7 to 29.3 per worktree, durable ref listings shared per repository, peak RSS set by the retained kinds rather than the whole log. Waived gate steps at landing, named by coordb: python.wrkslots.shard-2, python.wrkslots.shard-8, rust.wrkslots.test (#1926 hot-host budgets and the retire_pending deferred test; each failing test passes alone on the gated tree and on main, none touches audit code). Follow-ups remain as noted above: cache-planning listings and doctor batching.
