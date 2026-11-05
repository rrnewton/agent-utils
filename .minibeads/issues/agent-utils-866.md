---
title: 'latency-tests-cpu-budget: browser latency checks budget main-thread time tightly and wall time at ~8x, per the owner rule'
status: open
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T00:00:00+00:00
updated_at: 2026-11-05T00:00:00+00:00
---

# Description

Browser latency checks keep flaking in full gates and passing solo: `vibe-talk.check.fold-latency-browser` failed first-run twice in 6 h (gate ledger 2026-11-04 18:55Z, 22:22Z) and `vibe-talk.check.scroll-stability-browser` once (2026-11-05 02:29Z). Both judge wall-clock budgets measured in a real Chromium on a shared host, so contention turns healthy pages red. This is the #827 gate-flake-rate family, under the owner's #725 rule: CPU-time limits bite (~1.5x measured), wall-clock limits are forgiving (~8x the CPU budget).

# Design

- Measure the fold handler's main-thread time (the keydown task from capture to bubble, via performance marks; report long-task entries alongside) and judge it against a tight CPU budget of ~1.5x the measured solo median.
- Relax every wall-clock budget in the check (key to next frame) to ~8x the CPU budget, derived from it.
- Keep a hard shape: a genuinely slower handler (several times the work) still fails at any load.
- Review scroll-stability-browser the same way: its scroll walk is driven by scrollTop with no keys, so identify which of its budgets are CPU-like and which are wall, and apply the same split.

# Acceptance Criteria

- fold-latency-browser passes solo and under synthetic CPU load, and still fails with an artificially slowed page handler.
- The printed report shows the measured main-thread times, the CPU budget, and the derived wall budget.
- scroll-stability-browser gets the same review: either the same CPU/wall split or a recorded reason it does not need one.
- Gate passes; the #827 ledger keeps recording first-run outcomes.

# Notes

Fold handler solo baseline (2026-11-04, this host): handlers 11-29 ms (median ~14) against the old 50 ms wall budget; key-to-frame 17-63 ms against 120 ms.
