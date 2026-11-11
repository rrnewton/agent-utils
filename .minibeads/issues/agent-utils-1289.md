---
title: 'turnover-downgrade-guard: refuse a candidate that does not descend from a running commit'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-11T12:00:00+00:00
updated_at: 2026-11-11T12:00:00+00:00
closed_at: 2026-11-11T12:00:00+00:00
---

# Description

[watermelon-full-preview] From the #1220 services-turnover-timer-gaps
install review: the daily timer's `--from` checkout can go stale (its
HEAD is whatever was checked out when the host was set up, while the
local mirror fast-forwards `main` every few minutes). Turnover deciding
against a stale candidate would refresh units *backwards* to older
code. Today a unit whose running commit is ahead of the candidate is
decided `ahead` and simply not refreshed, while the rest of the run —
including the global binary — can still move onto the stale candidate.

# Design

- Before anything moves, a turnover apply run refuses when any
  unpinned unit (or the global binary) runs a commit the candidate
  does not descend from: exit 75, the run-failed notify phase fires,
  and the report names each such unit, its running commit, and the
  candidate, with the remedy (point `--from`/`--revision` at current
  main, for example the local mirror).
- A `--report` with an ahead unit exits 75 as an error, not 74.
- A pinned unit is exempt: its pin already takes it out of scope.
- A dry run still prints the plan and names the refusal it would make.

# Acceptance Criteria

- An apply run whose candidate is an ancestor of a unit's running
  commit moves nothing, exits 75, and names the unit and both commits.
- A report in the same state exits 75 with the ahead decision shown.
- Existing turnover suites stay green.

# Notes

Sequenced with the #1220 timer install on the first host: the timer's
ExecStart uses the local mirror (`--from .../mirror/agent-utils.git
--revision main`, fast-forwarded from GitHub main every 2 min), and
this guard is the backstop if that source ever stalls.

## Resolution

[watermelon-full-preview] Delivered with this closing commit.
`services turnover` computes the ahead units after pins are applied;
an apply run with any ahead unit (or ahead global binary) moves
nothing, exits 75, names each unit with its running and candidate
commits, and sends the run-failed notify phase. A report counts ahead
as an error (75). A test drives a unit whose binary reports the
candidate's descendant commit with the candidate set to its ancestor,
in both report and apply modes. Unverified: no live downgrade was
attempted; the guard was proven against fixtures and the mirror
--report/--dry-run in the install plan.
