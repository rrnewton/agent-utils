---
title: 'wrkslots-proof-mirrors-storage-dir: in the nested layout the proof-mirror directory is not a storage entry, so status and doctor report agent:proof-mirrors as directory-without-row and unpushed refuses on it (both editions)'
status: in_progress
priority: 1
issue_type: bug
assignee: cl-proofmirrors
labels:
- wrkslots
- census
created_at: 2026-11-28T05:35:00-05:00
updated_at: 2026-11-28T05:35:00-05:00
---

# Description

Found by an old-reader probe for #2392. It is visible to consumers.

In the default nested layout the control directory sits beside the agent slot directories. The control-directory entries that hold storage rather than being slots are listed once per edition: `_REPRESENTATION_DIRECTORIES` in `py/wrkslots/cli.py` (slot images, `slot-state`, `box-state`, the local salvage directory), and the matching list in the Rust status census (`rs/wrkslotsd/src/storage_state.rs`). The proof-mirror collection directory `<control>/proof-mirrors` added by #1692 is missing from both lists.

So after any removal that creates a proof mirror:

- `wrkslots status` and `wrkslots doctor` report `agent:proof-mirrors` as `directory-without-row`;
- `wrkslots unpushed` refuses, naming `agent:proof-mirrors` as an unregistered slot directory;
- audit lists it as an unregistered slot.

This holds on main and on the release a consumer has pinned.

# Design

Add `proof-mirrors` to the storage-entry set in both editions. In Python, add one name constant (`_PROOF_MIRROR_DIRECTORY`), use it in `_REPRESENTATION_DIRECTORIES` and at every place that spells the directory name today (the proof-mirror root, retirement, the clean-caches skip). In Rust, add the name to the status census list.

Nothing else changes. No persisted field is added or changed: this only changes how an existing directory is classified. One side effect: in the nested layout, an agent slot named `proof-mirrors` is now refused as reserved for wrkslots storage, as the other storage names already are. Without that, such a slot would share the directory with the proof mirrors.

The sandbox private-state masks (`py/wrkslots/sandbox.py`, `rs/wrkslotsd/src/sandbox_state.rs`) are a separate list (directories hidden inside a box). This issue leaves them unchanged.

# Acceptance Criteria

- In a nested-layout project, after a removal that creates `<control>/proof-mirrors`, `status` and `doctor` report no `directory-without-row` for it, and `unpushed` does not refuse on it (Python edition).
- The Rust status census skips the directory too, covered by a Rust unit test.
- Each new test is a negative control that fails on the base commit.
- Focused wrkslots tests pass, then the gate.

# Notes

Reported by the #2392 census lane. To be landed separately from #2392, after one review by the consumer's review seat before the gate.
