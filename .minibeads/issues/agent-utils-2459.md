---
title: 'wrkslots-remove-item-wallclock-budget: the remove-agent-batch per-item wall-clock budget refuses an ordinary agent-slot remove on a hot host'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- hot-host
- remove
created_at: 2026-11-28T07:53:00-05:00
updated_at: 2026-11-28T07:53:00-05:00
---

# Description

The Python edition of `wrkslots remove` gives each remove-agent-batch item a WALL-CLOCK budget for its work before the first deletion. Ordinary remove of an agent slot runs as a batch of one, so it has the budget too.

- The budget is `_AGENT_REMOVE_BATCH_ITEM_SECONDS = 60.0` (`py/wrkslots/cli.py:1205` on 2026-11-28 main). It bounds how long the item holds the registry lock before deletion, because other clients wait for that lock on their own finite bounds (a heartbeat waits 110 s). The clock starts when the item takes the lock.
- It grows 15 s per nested submodule, up to 600 s (`_AGENT_REMOVE_BATCH_ITEM_PER_SUBMODULE_SECONDS`, `_AGENT_REMOVE_BATCH_ITEM_MAX_SECONDS`, `cli.py:1211-1212`).
- Inside it, each in-process `/proc` scan gets 30 s plus 10 s per 1,000 host processes (`_PROC_SCAN_FLOOR_SECONDS`, `_PROC_SCAN_PER_THOUSAND_PROCESSES_SECONDS`, `cli.py:1044-1045`), held to what is left of the item budget. Every Git command, the liveness command, the lsof fallback and nested-repository judgment are held to it as well.
- When it runs out, `_item_budget_refusal` (`cli.py:19386`) refuses: "this remove-agent-batch item used its N-second budget for work before deletion at ...; nothing in the slot was deleted and the slot was kept". A refusal after the path fence rolls the fence back. It fails safe: nothing is lost, but the remove does not happen.

On a hot host the budget is spent by load, not by the slot. A host-wide census over thousands of processes, or salvage Git work, slows down with load1. The remove then refuses for a reason that names no fault in the slot.
- `cl-census` hit the 60-second remove-agent-batch budget at load1 about 250: the refusal came at the `/proc` scan after the path fence.
- Two tests are quarantined for this cause (`#2065`): `test_shared_submodules.py::test_remove_never_loses_index_only_blob_of_private_clone` and `::test_remove_keeps_every_linked_worktree_index_of_private_clone`. Their item budget runs out in salvage Git work under load, even inside a PID namespace, where the census itself is fast.

The Rust remove writer (`rs/wrkslotsd/src/remove_write.rs`) has no item budget; its only bound found is the lock wait (`ensure_states::lock_wait`). So the editions also differ here.

This conflicts with the owner's hot-machine rule: normal operations are designed for a host at high load, and only rare benchmarks need a quiet machine; cost bounds use load-invariant measures (CPU time, instructions).

# Design

The lead's direction: a CPU-primary budget, or none.
- CPU-primary: count the item's own CPU (its process and the commands it runs) rather than wall time. A loaded host then stretches the wall time without refusing, and a real runaway (a scan or command burning CPU) still stops.
- What still needs a wall bound is the registry-lock hold that other clients wait on. If a bound stays, it is a generous hang backstop, sized against the clients' waits, not against load. Alternatively the lock hold is restructured, so that long pre-deletion work (the census, salvage Git work) runs outside the lock and is rechecked cheaply under it.
- Keep the fail-safe shape: a refused item keeps its slot and names the cause.
- Today's workaround: `wrkslots remove --no-lock-budget` runs the process-use checks with lsof under the lock with no budget on the hold, and nested-repository judgment gets 900 s (`_NESTED_GIT_REMOVAL_SECONDS`).
- Both editions get the same rule, with a differential test.

# Acceptance Criteria

- An ordinary agent-slot remove on a host at load1 >= 237 completes, or refuses with a named cause that is not wall-clock (a live holder, a real runaway by CPU, a lock wait past a generous backstop). Evidence: runs with load1 recorded per run.
- The per-item budget, if any remains, is CPU-primary in both editions, with a differential test.
- The two `test_shared_submodules.py` cases come out of the load quarantine and pass in their namespaced shard in a gate.
- No wall-clock budget in a gating check (owner rule 346). No new persisted wrkslots field without the coordinator's reader-skew check.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number reserved by the coordinator), from the #2065 C1 analysis.

Related:
- #2065: the hot-host census tests; the two quarantined cases name this issue.
- #2418: the git helper race; a different hot-host remove refusal.
- #954: the per-submodule growth of the item budget.
