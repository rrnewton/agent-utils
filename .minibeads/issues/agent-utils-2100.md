---
title: 'wrkslots-rehome-previous-machine-identity: re-home rows recorded under a previous identity of the same machine'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T07:02:00-05:00
updated_at: 2026-11-22T07:02:00-05:00
---

# Description

A host's machine id changed at a reboot. Rows written before it carry the old stable host id, and wrkslots refuses to retire or remove them ("validation row ... belongs to stable host <old>, not current host <new>"). On one consumer host, more than half of the active rows (agent and validate, some without a checkout) are stranded this way, and there is no sanctioned way to clean them up.

# Design

Add an explicit re-home operation for rows from a previous identity of the SAME machine, gated on proof: the same hostname and the same recorded paths; the old id never observed live since the current boot (no heartbeat, lease or lock written under it after boot time); and an operator-confirmed old id. Re-homing records an auditable event naming both ids, then the ordinary retire and remove paths apply. Never re-home a row whose old identity could belong to a different live machine sharing the registry. Python and Rust editions agree. Any new persisted field or event kind is reported to consumers with older readers before landing.

# Acceptance Criteria

- Tests for: re-home succeeds with proof; refused for a different hostname, for differing paths, and when the old id was seen live after boot; retire and remove work after re-home; Python and Rust agree.

# Notes

Reported by a consumer's lead from another harness on the same host; filed by the coordinator.

[watermelon-full-preview] mu-rehome: implemented by extending the #1933 host-local alias (`wrkslots host-alias add`), not a new command and not a registry event. No new persisted field, event kind, or journal key: the alias file keeps schema 1 with the same entry keys, and readers without alias support never open it, so they keep failing closed. `host-alias add` refuses with no evidence (no row in this shard under the previous identity), then proves, before writing, that every such row's paths match this configuration's layout and that no such row heartbeated at or after the current boot start (`/proc/stat` btime, 5-second skew margin, ambiguous refuses). The path proof establishes layout consistency only; it does not tell machines apart, including for another machine sharing the registry. The heartbeat/boot guards are the separators, and heartbeats are explicit, not periodic. The validation-row and absent-row judges in both editions accept an aliased identity only under the same per-row guards; the ordinary retire, remove, and recover paths then apply.

Consumer note: a host that already recorded the alias (for example on 2026-11-20) needs to do nothing beyond running a wrkslots build that has this fix. That alias never ran the add-time proofs; only the judge-time guards protect it. Do not re-run `host-alias add` for the same previous identity; it refuses duplicates by design. Run the ordinary retirement/removal commands and the stranded rows are accepted.
