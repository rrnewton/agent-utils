---
title: 'restore-slot-rebind: revive transfers the wrkslots slot owner to the revived harness'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T13:45:36.698017487+00:00
updated_at: 2026-11-09T20:41:24.484350571+00:00
---

# Description

Found during the 2026-11-09 host reboot. Every revived lane that owned a worktree slot had to rebind the slot by hand after it came back. The slot manager's adopt refuses a dead historical owner by design, so each lane needed a manual transfer-owner with its new harness pid and the recorded generation before it could work in its slot again.

The per-registry restore already holds boxed slots before revive and, for lanes it restores through one path, transfers the slot owner afterwards. The gap is coverage: an agent with a recorded slot that comes back through restore or a standalone revive must reliably end with the slot owned by the revived harness, using the harness's configured slot manager, never a bare name from PATH. When that transfer is missing or refused, the lane is up but cannot prove slot ownership, and the failure surfaces later as an unrelated slot refusal.

Impact: a reboot multiplies manual slot surgery by the number of slot-owning lanes, and a lane that skips it works in a slot it does not own until the next lifecycle command refuses.

# Design

After a successful revive (including a promoted launch), for every agent whose record names a slot, transfer the slot owner to the revived harness process with the recorded generation, through the configured slot manager only (environment override, then profiles configuration, then the team-local binary; never PATH). Use transfer-owner, never adopt: adopt binding an unowned slot and transfer-owner replacing a provably dead owner are different safety claims, and a reboot is the second one.

Run the transfer once the harness pid is pinned, inside the same restore or revive operation, and record the outcome per agent (transferred, already-owned, or refused with the slot manager's reason) in the restore report and the revive result. Drop only the restore's own reboot hold after a successful transfer; a human hold is never dropped by restore. A refused transfer does not un-revive the lane, but it is never silent: it counts as a named refusal for the host command's table and exit class, with the exact remedy command.

Resolve the slot from the recorded slot name against the harness worktrees root, not from a stored working directory that may already be inside the slot.

# Acceptance Criteria

- A slot-owning lane restored after a simulated reboot owns its slot under the new harness pid with the same generation, with no manual adopt or transfer step, and the report row says transferred.
- A standalone revive of the same lane performs the same transfer; a second revive while the harness is live reports already-owned and changes nothing.
- When the slot manager refuses the transfer, the lane stays revived, the report names the refusal and the remedy, and the host command counts it toward exit 74.
- The slot manager is resolved from the configured sources only; a stale copy on PATH is never invoked (coverage runs with PATH lacking the tool).
- A lane with no recorded slot restores exactly as before, with no slot-manager invocation.

# Notes

Scope gap (b) from the 2026-11-09 reboot restore. The stale on-PATH wording in help text belongs to the restore-core lane; this issue is the behavioural guarantee and its report field. Touches revive and restore slot handling in both agentctl editions; coordinate function-level overlaps with the restore-core lane before editing.

D3 (bind verification before revive): before reviving or promoting each slot agent, restore verifies from the wrkslots image status record plus a findmnt check that the slot's bind mount is present at the mounted location and that the recorded working directory is that mounted slot, not a bare mount-point directory or a stale original. The slot is resolved from the recorded slot name and slot_project, never from the stored working directory. A worktrees-image-not-mounted warning (and a per-slot slot-image-not-mounted warning naming the slot) is fatal for that slot agent the same way. An unverified slot lane is refused with the missing bind or mount named and counts toward exit 74; non-slot agents and a non-slot coordinator still revive. Implemented in restore (verify_slot_bind) with report field slot_bind; standalone revive transfer remains the coordinator-owned revive.rs edit named in the gate report.
