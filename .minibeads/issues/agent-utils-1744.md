---
title: 'wrkslots-validate-batch-seal-left-after-start-refusal: a refused batch leaves its seal behind and blocks the host'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T15:45:41-05:00
updated_at: 2026-11-16T15:45:41-05:00
---

# Description

[watermelon-full-preview] Field report (2026-11-16, HIGH: it blocked a host-wide cleanup the owner ordered the same day): `remove-validate-batch` sealed 8 slots at 20:28:44Z, then refused at 20:39Z with "configuration changed while this command was starting", WITHOUT clearing its `VALIDATE-BATCH-SEAL` journal. The stale seal blocked every later batch, and other lanes' `adopt`, until `wrkslots recover`.

The seal is published by `_seal_validate_batch_targets` inside `_remove_validate_batch` (`py/wrkslots/cli.py`); the batch's `finally` retires only the exact seal file it published, and only under `_mutation_locks`, whose own configuration check is the refusal quoted above -- a refusal raised while acquiring the cleanup lock (or anywhere between the seal write and the first deletion) strands the seal. A seal left by a batch that deleted nothing is not an interrupted removal; nothing needs recovery, and the next operation should never have met it.

# Design

- Audit every refusal path between the seal write (`_write_validate_batch_seal_journal` in `_seal_validate_batch_targets`) and the first deletion (`_cmd_remove` per item): the census budget, the shared census, each item's preflight, and the retiring `finally` itself, including failures while taking the cleanup lock.
- Each such path releases the seal it published (the exact file, by the ownership check `_validate_batch_seal_is_owned`) before refusing, restoring each sealed directory's mode exactly as the normal `finally` does.
- Once any item has actually deleted something, the seal stays for `recover`, as designed: a partially completed batch is an interrupted removal.
- The seal write itself stays atomic and crash-safe: a process killed mid-batch (not refused) still leaves its seal for the next batch or `recover`, unchanged.

# Acceptance Criteria

- One deterministic test per refusal path between seal and first deletion: the batch refuses with its original reason, no `VALIDATE-BATCH-SEAL` journal remains, every sealed directory's mode is restored, and a following `adopt` or batch is not blocked by a seal.
- A test that a refusal after at least one real deletion leaves the seal in place for `recover`.
- The existing crash-boundary tests (killed after the seal is written) still leave the seal, and the seal-ownership suite still passes.
- Python and Rust editions agree where the Rust edition has the path (seal recovery itself stays with the Python edition).

# Notes

Related: #1736 wrkslots-live-seal-reported-as-interrupted (the seal the stranded journal leaves behind is at least named correctly now), #1735 wrkslots-remove-holds-state-lock-through-salvage.
