---
title: 'wrkslots-hooks-per-slot-type: post-provision hooks have no per-slot-type selection or skip condition'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

A consumer's post-provision hook (a dependency-regeneration step) now runs for agent slots too and fails on a proxy refusal, so create refuses. Recover also requires the coordinator pid in ancestry, so a lane cannot abort its own half-made create.

# Design

Let hooks declare which slot types they apply to (or a skip condition), and let a lane recover or abort its own half-made create without the coordinator's pid.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.

## Implementation notes

Per-slot-type hook scoping already exists (#1122): a
`post_provision_hooks` entry may be `{command, slot_types}`, and
`_post_provision_hooks_for` runs only the entries naming the new
slot's type; `common/docs/wrkslots/USER_GUIDE.md` documents it. The
missing part is the second half of the description: recover could
only run under the named coordinator's ancestry, so a lane with a
half-made create and no live coordinator (or an unrelated one) could
not roll that create back on its own. `recover --slot X
--abort-create` may now omit `--coordinator-pid`: `_cmd_recover`
routes to `_cmd_recover_abort_own`, which proves the caller descends
from the recorded create journal's owner (process identity via
`_assert_caller_process`, so a sibling lane cannot pass) before
rolling the create back. Failures fall back to the coordinator route;
`--coordinator-pid` is otherwise optional at parse time and recover
refuses with the omission rule named. Covered by
`py/wrkslots/tests/test_recover_abort_own.py`.
