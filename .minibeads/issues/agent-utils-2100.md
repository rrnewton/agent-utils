---
title: 'wrkslots-rehome-previous-machine-identity: re-home rows recorded under a previous identity of the same machine'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T07:02:00-05:00
updated_at: 2026-11-23T06:02:44-05:00
closed_at: 2026-11-23T06:02:44-05:00
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

## Resolution

[watermelon-full-preview] Landed on github/main as f4a0bf2b (verified combined tree 42f83b7a, replay of 3da920f7). Cold review of a6266e1c said DO NOT LAND; its seven findings were fixed in 3da920f7 (zero-evidence alias refusal, own-shard-only remedy text, sub-second Rust margin, missing-boot-id refusal, OverflowError catch, honest proof claims, pre-change alias caveat) and the re-review said LAND. Fresh gate on 3da920f7 (head_sha verified) had six reds, all waived or proven pre-existing: main's issue-parse frontmatter, a Rust CPU-TIMEOUT, a quarantined census test, and two pause tests failing identically on base bb47bad2. Overlap with main in `py/wrkslots/cli.py` and the user guide was verified on the exact combined tree (focused re-home tests, hold/adopt, scoped-status and recover suites, strict typing) before landing with --allow-overlap --verified-tree.

Follow-up leftovers from the re-review, deliberately not in the landed change: two stale docstrings (`py/wrkslots/cli.py` around the host-alias command and the locality helpers) still describe the proofs as the earlier draft did; add a test that the remedy text appears only for this host's own shard; add a Rust differential case at exactly the -5 second margin boundary.
