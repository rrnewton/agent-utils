---
title: 'wrkslots-strict-load-refused-by-unrelated-stale-row: create and other writers are refused host-wide by one stale row whose source repository is gone'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-27T13:05:08-05:00
updated_at: 2026-11-27T21:07:26-05:00
---

# Description

Sibling of #2396 (heartbeat), with the same cause. When one ACTIVE row names a source repository clone that was deleted, `wrkslots create` for a brand-new, unrelated slot exits 3 with "source repository '<path>' does not exist or cannot be resolved", naming the OTHER row's repository. Every command whose state load requires repositories fails the same way for every slot on the host. One stale row blocks new slots and per-slot mutations host-wide until someone repairs or removes it, and that repair can itself be refused.

# Design

Cause: every repository-requiring state load resolves EVERY active row's source repository. `_load_active_snapshot` / `_load_active` / `_load_all_active` / `_validate_global_state` with the default `require_repository=True` reach `_active_from_obj` -> `_assert_record_paths(require_repository=True)` -> `_stored_repository_path`, which raises for whichever row's clone is gone. Registry-wide storage scans do the same through `_assert_record_registered` / `_registry_storage_inconsistencies`, raising instead of recording a per-row finding.

Fix shape, as in #2396: a per-slot operation resolves only its OWN rows' repositories (the rows it creates, changes or removes). Every other row is validated lexically (the existing `require_repository=False` path keeps the containment checks: no symlink components, inside the project or one sibling, outside the managed worktrees), or lazily, or reported by name; never a refusal of this command. Storage scans record a missing repository as a finding for that row, named, instead of raising for the whole scan. A command that targets the stale row still refuses, naming the row and its missing repository. Python and Rust editions behave identically.

Survey every caller that loads with `require_repository=True` (create, register, import-existing, adopt, transfer-owner, hold and unhold, write-handoff, release, remove and retire of OTHER slots, and the rest) and classify each: per-slot (narrow it to its own rows) or deliberately fleet-wide (keep strict, but name the broken row and give a remedy).

# Acceptance Criteria

- A deterministic regression test: with one ACTIVE row's source repository deleted, `create` of a new slot succeeds. The test fails on the base commit.
- The same scene covers each other per-slot writer the survey finds (at least adopt, hold and transfer-owner of an unrelated slot): all succeed.
- A command that targets the broken row still refuses, naming that row and its missing repository.
- doctor and status keep reporting the broken row by name.
- Python and Rust parity covered by the differential tests.
- No new persisted fields.

# Notes

Found while tracing #2396, on 2026-11-27: with one stale row, heartbeat and create of unrelated slots were refused (rc 3), while status and doctor, which already load leniently, still ran. Scope here is everything except heartbeat, which #2396 fixes first.

## Parity call from #2396 (2026-11-27)

#2396 (landed 45a7c774) made only the heartbeat tolerant of another row's missing repository. In Rust the tolerant census variants in hold_write.rs (refuse_target_storage_except_nested_tolerant, report_dead_findings_tolerant) have one caller, the heartbeat; hold and transfer-owner deliberately keep the strict ones (refuse_target_storage_except_nested, report_dead_findings), and in Python their loads stay strict. So today both editions refuse hold and transfer-owner of an unrelated slot beside a stale row, identically, as they refuse create. This issue owns those writers: move each per-slot writer to a strict check of its own row plus the tolerant census, in both editions in the same change, so the editions never diverge on which commands a stale row blocks.
