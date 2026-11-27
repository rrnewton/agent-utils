---
title: 'wrkslots-heartbeat-refused-by-unrelated-stale-row: one stale ACTIVE row whose source repository is gone refuses every heartbeat on the host'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-27T12:57:06-05:00
updated_at: 2026-11-27T12:57:06-05:00
---

# Description

`wrkslots heartbeat` for a healthy slot exits 3 with "source repository '<path>' does not exist or cannot be resolved" when some OTHER ACTIVE row (a stale validate row) names a source repository clone that was deleted. A consumer saw every heartbeat on the host refused, for slots that have nothing to do with that row. One stale row turns into a host-wide heartbeat outage: owners stop looking alive, and liveness-driven reclaim can follow.

A heartbeat changes one row. It must judge only the heartbeating slot's own row and the repositories that row names. A broken row elsewhere must fail closed by name for that row (when a command targets it, and in doctor/validate reports), never for every heartbeat on the host.

# Design

Find where the heartbeat path resolves every row's source repository: the whole-fleet validation in the lock-free prepare and in the locked fallback, or a load-all-rows helper that resolves repositories as it parses. Then narrow it:

- The heartbeat judges its own record in full: owner, storage, the repositories it names, owner release, and the validate-batch seals and cleanup journals that overlap it.
- Other rows are read for what the heartbeat genuinely needs, for example disjointness from seals and journals. Resolving their source repositories is not one of those needs, and a failure to resolve one does not refuse this heartbeat.
- The stale row stays refused by name: any command that targets it (finish, remove, validate, doctor) still names it and its missing repository.
- Python and Rust editions behave identically.

# Acceptance Criteria

- A deterministic regression test with two slots. One ACTIVE row's source repository is deleted; a heartbeat of the OTHER slot succeeds (rc 0, heartbeat_at advanced). The test fails on the base commit.
- In the same setup, a command that targets the broken row is refused, and the refusal names that row and its missing repository.
- Python and Rust parity covered by the differential tests.
- No new persisted fields.

# Notes

Seen on a consumer host on 2026-11-27: one stale validate row, whose source clone a cleanup had deleted, refused all of that host's wrkslots heartbeats with rc 3. Sibling of #2353 (heartbeat busy fallback scan under the lock) and #2174 (transfer-owner).
