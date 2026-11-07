---
title: 'wrkslots-transfer-owner-after-restart: transfer-owner refuses after an in-place harness restart instead of warning and proceeding'
status: closed
priority: 1
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T22:36:35.000000000+00:00
updated_at: 2026-11-07T09:15:00.000000000+00:00
closed_at: 2026-11-07T09:15:00.000000000+00:00
---

# Description

On 2026-11-06 every agent on two hosts was restarted in place: same pane, same session, new harness process. Each lane then has to move its slot's owner to its new harness PID. `wrkslots transfer-owner` refused in two situations, and in each the slot's owner record stayed on the dead pre-restart PID:

1. **Nested linked worktrees.** A consumer slot holds nested linked git worktrees of its own repository under `ignored/` (five of them, some on no branch and dirty, holding live work) with no checkout rows. transfer-owner refuses because of them. A change of owner does nothing to those worktrees.
2. **A released generation reused by a later lane.** On this harness, three slots (`validate-speed2`, `refactor2`, `refactor-voice`) are at generation 1, released days ago by an earlier lane, and left in place because removal is frozen. A new lane went on working in each. transfer-owner refuses: "generation 1 was released by its owner; a released generation is waiting for removal and is not transferred". recover-unbound-owner does not apply either, because the slot has a recorded (dead) owner. The working lane has no supported way to become the slot's owner.

# Design

- Case 1: warn and proceed. List the unregistered nested worktrees, including dirty or unbranched ones, in the output and in the journal, then transfer. Ownership does not touch them, and removal keeps refusing as it does today.
- Case 2: provide a supported route for a coordinator to re-bind a released, unremoved generation to a live lane. For example `adopt --from-released --coordinator-authorized` starting a new generation that keeps the checkout, or a documented `transfer-owner --reopen`. Record who authorized it.
- Restart helpers name the right command in either case.

# Acceptance Criteria

- A test: a slot with unregistered nested linked worktrees transfers to a new PID with a warning listing them.
- A test: a released, unremoved generation is re-bound to a live lane through the new coordinator-authorized route; without authorization it still refuses.

# Notes

Reported 2026-11-06 by the lead coordinator (case 1, from a consumer lane) and seen on this harness (case 2) during the coordinated restart. P1 because every restarted lane needs transfer-owner.

## Resolution

Landed as 41b7daa7 with eda72e07 (typing fix in the Rust tests), by mu-wrkslots (watermelon), replayed by the coordinator onto current main.

- Case 1: `transfer-owner` warns and proceeds when a slot holds unregistered nested linked worktrees. It lists them, dirty or unbranched ones included, on stderr and in the event evidence. Removal still refuses them.
- Case 2: `transfer-owner --reopen --coordinator-authorized --coordinator-pid <pid> --reason <why>` rebinds a released, unremoved generation to a live lane as generation+1, recording the coordinator, the reason and the release sequence. It refuses without authorization, and the plain transfer refusal names `--reopen`.
- Same behaviour in the Rust wrkslotsd. USER_GUIDE updated. Acceptance tests: py/wrkslots/tests/test_owner_transfer.py and py/wrkslots/tests/test_transfer_owner_write_rust.py (both cases).

Validation: full gate on the first head plus solo reruns of its failed steps. A targeted gate (8 wrkslots shards, rust.wrkslots.test, python-types) passed 12/12 on each of three trees: the original base, rebased over d06c9e90, and the final replay over e79bd704.

This file was restored here after a renumbering commit elsewhere (a3ab4115) removed it by mistake. #983 was restored in the same commit.
