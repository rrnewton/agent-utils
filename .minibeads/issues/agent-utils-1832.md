---
title: 'wrkslots-prevalidated-starvation-under-churn: whole-registry identity retries can be starved by unrelated writes'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-18T00:05:24.000000000-05:00
updated_at: 2026-11-18T00:05:24.000000000-05:00
---

# Description

Prevalidated mutations (_run_prevalidated_mutation, py/wrkslots/cli.py:7082) commit only when the whole-registry identity captured before the lock-free prepare still matches under the locks, with REGISTRY_VALIDATION_ATTEMPTS=5 rounds (:6930) before a busy refusal with nothing written (:7182). Any intervening write — another slot's heartbeat, a snapshot resync, a hold toggle — invalidates a round, so under sustained registry-wide churn an adopt/create can exhaust its budget every invocation even though the commit is perfectly reachable in quiet moments. Exhaustive search in #1828 wrkslots-transition-model-check exhibits the starvation trace (report section 6) with a single heartbeating neighbour. This is a liveness note, not a correctness defect: the busy refusal is the designed safe outcome. Cross-links: #1276 adopt-lock-narrow (the prevalidated shape's origin), #1808 and #1829 stage 1 (config-lock contention work named by the coordinator; neither is in .minibeads at this base).

# Design

Keep the identity discipline and the bounded rounds; fix the retry economics. Options in increasing scope: document and implement jittered whole-command retry at the known busy-refusal callers; or wait briefly for a stable identity (event-driven on control-directory change, as the seal wait already does) before the final round so one invocation can ride out a heartbeat burst; or narrow the compared identity for slot-scoped mutations where the commit provably needs only its slot's files (the direction #1276 already takes). No persisted-format change. Coordinate with #1829 stage 1 if it touches the same wait paths.

# Acceptance Criteria

A deterministic churn harness (a neighbour heartbeating on a condition-driven loop, no sleeps-as-sync) shows adopt/create completing within a bounded number of whole-command attempts at a stated heartbeat rate where it previously exhausted every time, and the busy refusal still fires with nothing written when the bound is exceeded. The model witness in py/wrkslots/tests/test_transition_model.py (WRKSLOTS_MODEL_FULL=1) stays as the shape reference.

# Notes

[watermelon-full-preview] Filed from #1828 wrkslots-transition-model-check at the coordinator's request; see ai_docs/reports/research/2026-11-18-wrkslots-model-check.md section 6.
