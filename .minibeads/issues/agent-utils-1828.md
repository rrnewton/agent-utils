---
title: 'wrkslots-transition-model-check: an executable model of registry transitions, locks and crashes that finds wedges'
status: open
priority: 1
issue_type: task
assignee: mu-modelcheck
labels:
- wrkslots
created_at: 2026-11-17T22:44:40.000000000-05:00
updated_at: 2026-11-17T22:44:40.000000000-05:00
---

# Description

Two wrkslots wedges were found the hard way on 2026-11-17: #1766, where remove-validate-batch deleted a subvolume and only then discovered a foreign git-worktree lock, leaving journals that blocked every exit; and #1807 wrkslots-remove-stale-active-revision, where a prepared remove committed from a lock-free stale registry state while another slot's heartbeat landed, forking the event-log revision chain and wedging every command on the registry. An executable model of the persistent state (event-log revision chain, ACTIVE records, journals, the config flock and per-file locks, validation-batch seals, heartbeat TTLs, holds) with concurrent actors and crash injection at every lock acquire/release and every append point should find this class of broken transition mechanically instead of in production. Analysis only: no product-code changes in this lane; confirmed new wedges are reported as candidate issues for the coordinator to file.

# Design

Bounded explicit-state checker alongside the #1568 single-slot lifecycle model, using the same checked-abstraction pattern: a Python BFS explorer with a sticky violation ghost, scenario-constant fix/bug toggles and shortest-trace counterexamples, and no downloaded toolchain (no TLC is available offline; the #1568 precedent already chose the Python explorer as the gated checker for that reason, with an optional TLA+ text mirror). Actors are small programs with explicit program counters whose phase order (lock-free read, salvage, lock acquire, revalidation, append, ACTIVE write, unlock, journal publish/retire) is transcribed from py/wrkslots/cli.py, each step citing file:line. Bounds are stated up front (2 slots, up to 3 concurrent actors, bounded revisions and clock ticks); crash is an environment transition available at every phase boundary that abandons the actor while keeping its journals and releasing its locks. Searches check revision-chain continuity, ACTIVE-equals-replay, one owner per slot, never archived-and-active, journals eventually resolvable, and no reachable all-commands-refuse state reachable from a healthy one.

# Acceptance Criteria

Without encoding the answers as assertions, the checker rediscovers #1807 on the pre-fix code path (current main: the fix sits unlanded on mu-removerace/1807) and #1766 on its pre-fix phase order, each as a minimal counterexample trace quoting the production refusal; toggling the corresponding fix constant makes those traces unreachable. Every other counterexample is reported with a minimal trace, the code path (file:line), a proposed fix, and a candidate-issue draft in the report (not the tracker). Deliverables: model and checker under py/wrkslots/ with pytest coverage, a README on how to run it and what it covers, and ai_docs/reports/research/2026-11-18-wrkslots-model-check.md stating the bounds and what is not modelled. Deterministic tests only; condition-based waits; no product-code changes.

# Notes

[watermelon-full-preview] Owner-requested model-checking exercise (2026-11-17). First cut (model scope plus whether #1807 is rediscovered) is reported to the coordinator before the search goes wide. #1568's py/wrkslots/lifecyclemodel.py covers single-slot lifecycle safety/liveness; this issue adds the multi-actor registry/revision/lock/crash dimension it deliberately folded away (cross-slot effects there are two scenario flags).
