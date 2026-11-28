---
title: 'wrkslots-proof-mirrors-storage-dir: in the nested layout the proof-mirror directory is not a storage entry, so status and doctor report agent:proof-mirrors as directory-without-row and unpushed refuses on it (both editions)'
status: closed
priority: 1
issue_type: bug
assignee: cl-proofmirrors
labels:
- wrkslots
- census
created_at: 2026-11-28T05:35:00-05:00
updated_at: 2026-11-28T07:45:00-05:00
closed_at: 2026-11-28T07:45:00-05:00
---

# Description

Found by an old-reader probe for #2392. It is visible to consumers.

In the nested layout the control directory is the agent slot directory, so its entries sit beside the slots. This applies to a project whose `worktrees_dir` does not end in `slots` and whose layout is nested or unset. `init`'s default `worktrees/slots` puts the control directory one level up, so default projects and flat projects are not affected. The control-directory entries that hold storage rather than being slots are listed once per edition: `_REPRESENTATION_DIRECTORIES` in `py/wrkslots/cli.py` (slot images, `slot-state`, `box-state`, the local salvage directory), and the matching list in the Rust status census (`rs/wrkslotsd/src/storage_state.rs`). The proof-mirror collection directory `<control>/proof-mirrors` added by #1692 is missing from both lists.

So after any removal that creates a proof mirror:

- `wrkslots status` and `wrkslots doctor` report `agent:proof-mirrors` as `directory-without-row`;
- `wrkslots unpushed` refuses, naming `agent:proof-mirrors` as an unregistered slot directory;
- audit lists it as an unregistered slot.

This holds on main and on the release a consumer has pinned.

# Design

Add `proof-mirrors` to the storage-entry set in both editions. In Python, add one name constant (`_PROOF_MIRROR_DIRECTORY`), use it in `_REPRESENTATION_DIRECTORIES` and at every place that spells the directory name today (the proof-mirror root, retirement, the clean-caches skip). In Rust, add the name to the status census list.

Nothing else changes. No persisted field is added or changed: this only changes how an existing directory is classified. One side effect: in the nested layout, an agent slot named `proof-mirrors` is now refused as reserved for wrkslots storage, as the other storage names already are. The same check guards `register`, `import-existing` and the recovery of an interrupted create or import. Without the refusal, such a slot would share the directory with the proof mirrors, and proof-mirror retirement would delete its checkouts (see Notes).

The sandbox private-state masks (`py/wrkslots/sandbox.py`, `rs/wrkslotsd/src/sandbox_state.rs`) are a separate list (directories hidden inside a box). This issue leaves them unchanged.

# Acceptance Criteria

- In a nested-layout project, after a removal that creates `<control>/proof-mirrors`, `status` and `doctor` report no `directory-without-row` for it, and `unpushed` does not refuse on it (Python edition).
- The Rust status census skips the directory too, covered by a Rust unit test.
- Each new test is a negative control that fails on the base commit.
- Focused wrkslots tests pass, then the gate.

# Notes

Reported by the #2392 census lane. To be landed separately from #2392, after one review by the consumer's review seat before the gate.

Why the reserved name matters (found in review). On the base code, a slot named `proof-mirrors` in an affected project was in danger, not just untidy. `_proof_mirror_retirement_candidates` treats every entry of `<control>/proof-mirrors` as a mirror. A checkout directory has no `record.json`, so it reads as idle (`fetched_at` 0), and its name is not a live remote digest, so it also reads as orphaned. `_retire_proof_mirrors` then runs `shutil.rmtree` on it. So the next `clean-caches --yes` would have deleted that slot's checkouts. On the base code, wrkslots' own remedy text for the directory-without-row finding even suggested `wrkslots register proof-mirrors`. The refusal closes this for new slots. It does not protect a slot that already has the name; none was found on the hosts checked.

Residual, unchanged: in an affected project, an unregistered real slot directory named `proof-mirrors` is now hidden from status, doctor, unpushed and audit, as for the other four storage names. Possible follow-ups, not filed here:

- Retirement could consider only 64-hex mirror names, and doctor could flag any other entry. Today a non-directory entry makes the `.lock` open raise.
- Rust create reserves only `validate`.
- The Rust audit enumeration has no storage-name filter.

The Rust coverage is a differential test that runs the real `wrkslotsd` binary against the Python oracle, the module's established pattern, rather than a Rust `#[test]`.

## Resolution

Landed as e51595a6 (fix) and bed697e3 (review follow-ups) on main, on top of 9bf112c0. The gate was path-selected on the gated head 6bdb9f21 and passed: 76 steps, none failed or incomplete. Main had changed `py/wrkslots/tests/test_lifecycle.py` since the gated base: one load-quarantine marker line, far from this change. So the replay landed with `--allow-overlap` after a combined-tree check on the replayed tree d94d7156 (a first check on b3f2bc7c went stale when main moved again). That check ran static checks, the issue parse, the two Rust differential files, the proof-batching tests and the focused lifecycle selection.

- Python: `_PROOF_MIRROR_DIRECTORY` is in `_REPRESENTATION_DIRECTORIES`. Status, doctor, unpushed, audit and clean-caches no longer treat `<control>/proof-mirrors` as a slot directory. A slot of that name is refused as reserved for wrkslots storage.
- Rust: the status census list (`REPRESENTATION_DIRECTORIES` in `storage_state.rs`) includes `proof-mirrors`. That covers Rust status, doctor and unpushed.
- Tests:
  - `test_proof_mirror_directory_is_storage_not_a_slot`: a real removal in the nested layout, then status, doctor, unpushed and the reserved name.
  - `test_parse_storage_storage_entries_are_not_slots_matches_python`: Rust against Python.
  - Both fail on the base commit.
- One review read by the consumer's review seat: OK with conditions, both met in the follow-up commit.

Not done here (follow-ups): retirement limited to mirror-digest names; Rust create reserved names and the Rust audit storage filter.
