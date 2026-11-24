---
title: 'wrkslots-lifecycle-fuzz: a TLA+ transition model checked with TLC, then seeded fuzz of the real registry'
status: closed
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-21T08:18:00.000000000-05:00
updated_at: 2026-11-24T18:09:15.000000000-05:00
closed_at: 2026-11-24T18:09:15.000000000-05:00
---

# Description

The Python explicit-state model (#1828, py/wrkslots/transitionmodel.py) checks the registry's concurrent transitions with a breadth-first explorer because no TLA+/TLC toolchain was available offline and validation must not download one. The owner asks why TLA+ cannot be downloaded and wants both real model-checker tooling and a connection from the model to random tests of the real code. #2041 shows the cost of the dimensions the Python model folds away: a finish journal held in memory in sidecar form (containment inventory over the 64-entry inline limit, #1757) never equals its reloaded expanded form, so the out-of-lock completion's identity compare refuses and leaves RECOVERY REQUIRED. Journal content and representation, per-slot versus global journals, and the out-of-lock windows are therefore first-class model citizens in this issue, not folds.

Phase 1 ports the model to TLA+ at the same abstraction (two slots, actors with program counters, revision chain, ACTIVE snapshot, journals, mutation lock, crashes) plus journal content where it matters, checks it with TLC at sensible bounds, and cross-checks both explorers on the known results (#1807 forked chain, #1766, C1/#1830, C2/#1831). Phase 2 connects the model to seeded random tests of the real wrkslots code and starts only when the coordinator confirms #2041 has landed.

# Design

Toolchain: download only tla2tools.jar from the official tlaplus GitHub release into a new directory outside every git tree, record version and sha256 (v1.7.4, sha256 936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88; that release publishes no checksum file), run TLC with the host Java 21. Validation never downloads and never runs TLC: the spec and an opt-in runner live in the repo (py/wrkslots/spec/WrkslotsTransition.tla, WrkslotsTransition-*.cfg, run_tlc.sh) and the Python model stays the gated cross-check.

Spec: plain TLA+, action names identical to the Python model, code variants as constants mirroring Scenario.fixes, plus RefsLarge (inventory over the inline limit) and NormalizeCompare (the #2041 fix shape: compare after expanding both sides). Invariants include the Python model's checks plus: every interrupted state is completed by one recover (liveness under recover, hand-exit and actor-exit fairness), no refusal on a quiescent consistent store, the event chain always loads or carries its violation ghost, and the reloaded journal equals the in-memory journal. The deferred out-of-lock deletion (#894/#1735) is an explicit window beside the prepared-remove salvage window.

Phase 2 (not started): a seeded reproducible harness generating random command sequences (create, adopt, heartbeat, fence, finish, remove, recover, transfer-owner, hold) with crash points between journal and event writes and concurrent operations from separate processes, run against the real wrkslots in a scratch registry with per-worker CPU, memory and pid containment; after every step the real state (events, ACTIVE, journals including sidecars) is decoded and compared with the model's prediction. A fast seeded subset gates validation (condition-based, robust on a loaded host); the long soak is opt-in. Every divergence becomes a minimized reproduction plus a regression test.

# Acceptance Criteria

Phase 1: TLC checks the spec at the stated bounds and the two explorers agree on #1807 (same interleaving order), #1766, the C1 shapes and the contention witness; TLC finds the #2041 class exactly when RefsLarge holds with the raw-form compare, and the class closes under NormalizeCompare. Invariants, distinct-state counts, depths and counterexample traces are reported to the coordinator. Phase 2: the harness runs seeded sequences against the real code under containment, decodes real state after every step, and reports every divergence as a minimized reproduction with a regression test; the fast subset passes in validation with condition-based waits only.

# Notes

[watermelon-full-preview] Owner request relayed by the coordinator (2026-11-21). Phase 1 first TLC results are recorded in py/wrkslots/TRANSITIONMODEL.md. Product-code fixes stay with the owning lanes (the #2041 compare sites are not touched here); this lane adds models, tests and harnesses. Phase 2 starts on the coordinator's #2041-landed signal. No landing without the coordinator's LAND NOW.

## Resolution

[watermelon-full-preview] Complete. Phase 1 landed 2b9e11b4 (TLA+ transition model, TLC cross-checks, #2041 sidecar class). Phase 2 landed e9fb82cb and 55a3e4c5 (seeded harness with model-as-oracle decode, crash seams, concurrent workers, fsync fault family pinned as #2049, exit-sweep loop-device teardown #2048, root-mount opt-in 88ee8c53). Transferred residuals: fast-seeds un-quarantine under #2167, orphan loop recovery under #2050, finer-grained lock modelling under #2090 (landed) and #2127.
