---
title: 'wrkslots-heartbeat-refused-by-unrelated-stale-row: one stale ACTIVE row whose source repository is gone refuses every heartbeat on the host'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-27T12:57:06-05:00
updated_at: 2026-11-27T21:07:26-05:00
closed_at: 2026-11-27T21:07:26-05:00
---

# Description

`wrkslots heartbeat` for a healthy slot exits 3 with "source repository '<path>' does not exist or cannot be resolved" when some OTHER ACTIVE row (a stale validate row) names a source repository clone that was deleted. A consumer saw every heartbeat on the host refused, for slots that have nothing to do with that row. One stale row turns into a host-wide heartbeat outage: owners stop looking alive, and liveness-driven reclaim can follow.

A heartbeat changes one row. It must judge only the heartbeating slot's own row and the repositories that row names. A broken row elsewhere must fail closed by name for that row (when a command targets it, and in doctor/validate reports), never for every heartbeat on the host.

# Design

Cause (traced on main and at a consumer pin 132dd98d): every repository-requiring state load resolves EVERY active row's source repository. `_load_active_snapshot` / `_load_active` / `_validate_global_state` reach `_active_from_obj` -> `_assert_record_paths(require_repository=True)` -> `_stored_repository_path`, which raises "does not exist or cannot be resolved" for whichever row's clone is gone. heartbeat loads that way in its TTL read, its lock-free prepare, its locked fallback and its post-write snapshot compare. Its lock-free prepare also runs the registry-wide storage scan (`_assert_command_registry_storage` -> `_assert_record_registered` / `_registry_storage_inconsistencies`), which resolves each row's repository the same way and raises instead of recording a per-row finding. The Rust edition refuses the same scene the same way (its global validation and storage-git scan).

Fix: the heartbeat path, and any per-slot operation, resolves only its OWN rows' repositories. Other rows are validated lexically (the existing `require_repository=False` path keeps the containment checks: no symlink components, inside the project or one sibling, outside the managed worktrees), or lazily, or reported by name; never a refusal of this command.

- The heartbeat judges its own record in full: paths and repositories resolved strictly, owner, its own storage (the one-row storage check plus the one-slot nested-registration scan from #2174/#2353), owner release, and the validate-batch seals and cleanup journals that overlap it.
- Other rows are read only for what the heartbeat genuinely needs, for example disjointness from seals and journals.
- The stale row stays refused by name: its own heartbeat, and any command that targets it, names it and its missing repository.
- Python and Rust editions behave identically.

# Acceptance Criteria

- A deterministic regression test with two slots. One ACTIVE row's source repository is deleted; a heartbeat of the OTHER slot succeeds (rc 0, heartbeat_at advanced). The test fails on the base commit.
- In the same setup, a command that targets the broken row is refused, and the refusal names that row and its missing repository.
- Python and Rust parity covered by the differential tests.
- No new persisted fields.

# Notes

Seen on a consumer host on 2026-11-27: one stale validate row, whose source clone a cleanup had deleted, refused all of that host's wrkslots heartbeats with rc 3. Sibling of #2353 (heartbeat busy fallback scan under the lock) and #2174 (transfer-owner). The same stale row also refuses `create` host-wide (rc 3); `status` and `doctor` load leniently and still run. Other strict loaders are probably affected too and are outside this issue unless widened.

## Resolution

[Claude Opus 5.5] Landed on main as 45a7c774. It was gated as 0a5bd9ef: the gate was red only on four wrkslots shards and rust.merge-queue.test, the exclusive rerun passed merge-queue and shards 2, 9 and 10, and the liaison waived shard 12 (a real-/proc scan timeout in test_unmount_busy_clears_on_a_bounded_retry, filed as #2434). It was replayed onto main with a verified combined tree; the only overlap was validation/components.json. A heartbeat now loads the registry lexically (require_repository=False) and resolves only its own row strictly, naming that row in the refusal, so the stale row's own heartbeat still fails closed by name. The scoped storage scan tolerates another row's unavailable repository, which the retained-storage warning already names. In Rust the heartbeat resolves only its own row's repositories, and its target-storage and dead-mount checks read a tolerant census variant. Parity call: the tolerant census is heartbeat-only. In both editions hold and transfer-owner keep the strict census and strict loads, so both still refuse those commands beside a stale row, the same way; they belong to #2397 with create and the other writers. Tests in test_heartbeat_unrelated_row.py (4): with the second slot's source clone deleted, the other slot's heartbeat succeeds in both editions (heartbeat_at advances; the stale row is named, retained and unchanged); the stale row's own heartbeat refuses in both editions, naming the row and its missing repository, with nothing written; an unscoped storage scan still refuses. No persisted fields.
