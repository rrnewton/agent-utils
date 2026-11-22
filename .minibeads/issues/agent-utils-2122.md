---
title: 'wrkslots-hold-tolerates-registration-drift: hold succeeds when a slot has an undeclared nested worktree'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T12:05:00-05:00
updated_at: 2026-11-22T12:05:00-05:00
---

# Description

wrkslots hold refuses (rc 3) on live slots that contain a nested Git worktree of the same source repository which the slot row does not declare: the source repository registers a worktree inside the managed agent root that no active checkout row names. A hold is the protective direction, so refusing it on an unrelated registration inconsistency fails dangerous. adopt-checkout also does not cover same-repository nested worktrees.

# Design

hold records the hold even when such registration drift exists, and reports the drift as a warning naming the path; refusals that would make a hold unsafe stay. Separately, give adopt-checkout (or a sibling) a way to declare a same-repository nested worktree inside a slot.

# Acceptance Criteria

- Tests: hold on a slot with an undeclared nested same-repository worktree succeeds with a warning; the slot is then never attempted for removal; adopt covers the nested case or the issue says why not.

# Notes

Reported by a consumer's lead; filed by the coordinator.

## Resolution notes (mu-holddrift)

Hold: fixed. `hold` (Python `_cmd_hold`, Rust `hold_slot`) now tolerates a
target slot's `git-registration-without-row` finding when the registered
path is present, recording the hold and printing a warning that names the
path, mirroring heartbeat (#1492) and transfer-owner. A registration whose
path was deleted still refuses, as do all other storage findings. No new
persisted field or event: the `slot-held` payload and HOLD record are
unchanged, so older readers are unaffected. Differential-tested
(`test_hold_write_rust.py`) plus lifecycle tests including that removal
never proceeds on the held, drifted slot.

Adopt-checkout: a same-repository nested worktree CAN be declared, but
only at the slot's top-level checkout directory (`<slot>/<name>`). That is
the only path a checkout row can name: the registry loader enforces
`checkout.path == slot/<name>` for every reader, so a worktree nested
deeper (inside another checkout, or under a subdirectory) cannot be
represented without changing the persisted row shape and every reader
that relies on the invariant. `adopt-checkout` now refuses such a path up
front with the remedy (`git worktree move` to `<slot>/<name>`, then rerun;
or leave it undeclared, since hold/heartbeat/transfer-owner now tolerate
it with a warning) instead of failing in the event-append check. Covered
by `test_adopt_checkout_declares_same_repository_nested_worktree` and
`test_adopt_checkout_refuses_deeply_nested_worktree_with_remedy`.
`adopt-checkout` remains Python-edition only in Rust (`wrkslotsd` stubs
it), so no Rust change was needed for this part.
