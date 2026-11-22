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
