---
title: 'wrkslots-state-lock-deletion: keep slow fence and deletion phases outside the ACTIVE registry locks'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
- state-lock
created_at: 2026-11-10T15:05:51.216194564+00:00
updated_at: 2026-11-10T15:05:51.216194564+00:00
---

# Description

Removal reports show the workspace-registry locks rooted at the control directory ACTIVE state held across slow pre-completion work. Fence repair and parts of deletion, including storage-volume deletion and synchronization, filesystem removal, and salvage follow-through, are not independently bounded there. One removal reportedly held a state lock for about nine minutes; during that interval, slot creation failed with its bounded-wait exit and an audit timed out after waiting 300 seconds. This is distinct from the project-root configuration lock handled by #1194 wrkslots-lock-scope: the question is every caller and removal shape that keeps the ACTIVE global and machine state locks while doing work that is not an atomic registry transition.

# Design

Map every removal path in py/wrkslots/cli.py, including ordinary remove, retire batches, private cleanup, scoped validation finish, recovery, and any fence-repair path. Hold the ACTIVE locks only to validate and durably record a transition or completion; release them while repairing a fence, deleting a fenced tree or storage volume, synchronizing storage, removing files, and doing salvage work that does not need registry exclusion. Reacquire and revalidate the exact slot generation, journal/fence identity, and salvage state before recording completion. If material state changed while unlocked, fail closed and preserve the slot and recovery evidence. Keep #1194 configuration-lock behavior unchanged.

# Acceptance Criteria

A fake slow fence repair, storage deletion, synchronization, file removal, or salvage step does not block slot creation or a shared registry read beyond a few seconds. Each removal shape still refuses or resumes safely when generation, journal, fence, or salvage identity changes while unlocked. Existing lifecycle, audit, census, and lock-scope tests pass. Report before/after maximum ACTIVE-lock hold times on a scratch project.

# Notes

Start with `_mutation_locks`, `_begin_finish`, `_finish_fenced_slot`, `_finish_remove_paths`, and every caller that retains the registry locks across those calls in `py/wrkslots/cli.py`. Ordinary deferred deletion is the useful precedent: its journal and fence transition are recorded under the locks, the fenced tree is then deleted unlocked, and the locks are taken again for completion. The follow-up should inventory where that boundary does not yet hold, especially fence repair, private cleanup, scoped validation finish, batch removal, and interrupted-operation recovery. The reported nine-minute hold, creation failure, and audit timeout are report evidence to reproduce on scratch state, not measurements made while filing this issue.

## Update (#1367)

[watermelon-full-preview] The remove-agent-batch shape of this issue is folded into #1367 wrkslots-salvage-thin-bundle-fast piece (2): batch items are validated and planned lock-free and commit one record at a time under the ACTIVE locks, following #1287. The remaining shapes named here (fence repair, private cleanup, scoped validation finish, interrupted-operation recovery) stay open on this issue.
[watermelon-full-preview] Supersession for remove (#1367 piece 2b): the "keep #1194 configuration-lock behavior unchanged" note above does not apply to remove. Ordinary remove now follows #1276 like create: the configuration flock is held only to read/validate config, never across salvage, census, or deletion.
