---
title: 'wrkslots-removal-ref-inventory: every removal records the refs it found, so a removal can be audited later'
status: closed
priority: 1
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-11T01:20:00+00:00
updated_at: 2026-11-11T02:11:06+00:00
closed_at: 2026-11-11T02:11:06+00:00
---

# Description

[opus 5.5] After #1255 wrkslots-private-history-refusal, a consumer audited six slots removed earlier and found it can't prove nothing was lost. The archive records keep each checkout's HEAD and salvage disposition, but not which branches and tags existed before removal. So slot-local history that was never recorded is unknown, not proven absent.

# Design

- Before deleting anything, removal writes a ref inventory into the archive record: for the checkout and for every submodule checkout, each ref name (refs/heads, refs/tags, refs/stash, refs/remotes, notes) with its target object ID, plus HEAD.
- The inventory is bounded: large ref sets are written to a sidecar file in the slot's salvage directory, with a count and digest in the record, so the record stays readable.
- No network access and no new refusals: the inventory is evidence only.
- Rust parity: same fields.

# Acceptance Criteria

- An archive record after removal lists every ref, with its object ID, for the checkout and each submodule.
- Tests count refs on fixtures (branches, tags, stash, a submodule with local refs) and check the record and the sidecar.

# Notes

Requested by the consumer that reported #1255. Related: #1255 (private history now refuses or is bundled) and #1257 (declared submodules' local refs).

## Resolution

[opus 5.5] Delivered for agent-slot removal (remove, remove-agent-batch, retire-pending), in both editions.
- Every salvage receipt (each checkout, and every submodule, unlisted and nested module repository) carries a `ref_inventory`, gathered after salvage and before the path fence. Receipts flow into the finish journal, the slot-removed event and ARCHIVED.<machine>.json records[].salvage[]. It lives inside the receipt, not as a new record key, because both editions' archive readers check record keys exactly, while salvage items are open mappings (the archived-state schema allows extra properties there). So older readers keep working.
- Shape: `scope` (`repository` when the Git directory is the common directory, so every ref is listed; `worktree` for a linked worktree, listing only refs/worktree/, refs/bisect/, refs/rewritten/), `head` (the symbolic ref, or null), `count`, `sha256` (of the "OBJECT REFNAME\n" lines in ref-name order), then `refs` ([REFNAME, OBJECT] pairs) when count <= 64, else `file`. The file is a sidecar wrkslots-salvage/<machine>/<slot>/<generation>/ref-inventory/<checkout>-<digest>.json under the salvage archive root, written and read back, holding the same fields plus the full list.
- Evidence only: no network, no new refusal. A failure is recorded as {"error": "..."}. A receipt that replaces a lost #1153 proof keeps the inventory.
- Rust parity: remove_write.rs salvage_gate writes the same `ref_inventory` for the clean top-level checkouts the Rust remove handles. The Rust and Python finish journals and archive records stay byte-exact in test_remove_write_rust.py. More than 64 refs needs the sidecar, which only Python writes, so the Rust remove refuses that corner ("stays with the Python edition for now").
- Not covered: the ownerless and absent-row recoveries and the frozen-release path (follow-up drafted for the coordinator).
- Tests: test_remove_records_the_ref_inventory_of_each_checkout_and_submodule (a branch, a lightweight and an annotated tag, the stash and a stale remote-tracking ref in a submodule; refs/worktree/* in the checkout; exact pairs and digests), and test_remove_writes_a_large_ref_inventory_to_a_sidecar (70 tags: sidecar path, pairs and digest).
