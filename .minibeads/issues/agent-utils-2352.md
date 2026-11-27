---
title: 'wrkslots-e2e-concurrent-writer-tmp-visibility: a heartbeat sees a sibling writer in-flight event tmp file under load'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-26T12:30:00+00:00
updated_at: 2026-11-26T12:30:00+00:00
---

# Description

[watermelon-full-preview] Found 2026-11-25 while gating #2168: `wrkslots/tests/test_e2e_stress.py::test_real_process_and_git_invariants` (seed 7) failed twice in full gates (python.wrkslots.shard-10) with a concurrent-stress heartbeat refusing "partial atomic update found: EVENTS.e2ehost/00000000000000000014.json.tmp.<rand>; preserve it and use 'wrkslots recover'". A heartbeat scans the slot's event directory while a sibling worker is between writing its event tmp file and renaming it into place, and treats the in-flight tmp as corruption. Retained evidence from the first failure: /tmp/wrkslots-e2e-7-1ru8158c (events 01-10 landed; 14 was mid-write).

# Design

Decide where the tolerance belongs: a heartbeat (a read-mostly liveness probe) arguably should ignore or retry past a tmp file that a live sibling is about to rename, while a recovery or audit path should keep refusing a stale tmp. Any tolerance must be bounded (a tmp older than some age is still corruption) and must not weaken the atomic-update check for committed event files. No new persisted field is expected; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- The exact test passes 5x alone on the fixed tree and on the base it was reported against, at recorded load, and in a full gate shard without the visibility refusal.
- A genuinely stale tmp (no live writer) still refuses with the existing remedy.
- Existing heartbeat/event suites pass.

# Notes

Triaged under the flake rule with 5/5 solo passes on both the reported tree and main (load1 115-154 recorded); cleared as a load-amplified race, not a product regression, and filed here as the real follow-up. Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.
