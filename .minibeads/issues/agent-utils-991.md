---
title: 'mb-create-atomic-allocation: reserve issue numbers against a shared source instead of the local checkout'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-07T01:30:00.000000000+00:00
updated_at: 2026-11-07T01:30:00.000000000+00:00
---

# Description

Follow-up to #951 mb-id-collision-guard, which added the landing guard
(`scripts/check_issue_numbers.py`): the guard refuses a bad landing, but
`mb create` still takes the next free number from the local checkout's
highest number, so two lanes on stale checkouts still collide and only
find out at the gate.

# Design

Options, weighed in #951:

- **Compare-and-swap reservation ref on the canonical remote (chosen):** `mb
  create` reads the current highest number from a small reservation ref (for
  example `refs/mb/next`), then pushes an updated reservation — the issue
  file itself, reserved under that number — with compare-and-swap semantics
  (`git push --force-with-lease` on the reservation ref, or a first-come
  object existence check: pushing `refs/mb/issue-<N>` fails if another lane
  already pushed it). The loser retries with N+1. The reservation lives in
  the same git remote agents already fetch through, needs no new service,
  and the reserve-then-file steps degrade to today's behaviour when offline
  (queue locally, guard still protects the landing).
- **Collision-proof suffixes (rejected here):** per-lane prefixes or short
  random suffixes make collisions structurally impossible but break the
  readable `#N slug` convention this repository runs on (issues, PRs and
  commit bodies all cite `#<number> <slug>`), and renumbering after the fact
  loses the stable number people already quoted.

# Acceptance Criteria

- Two `mb create` runs racing from equally stale checkouts end up with
  distinct numbers, and neither files over the other's reservation.
- Offline `mb create` still works and says the number is unreserved; the
  landing guard remains the backstop.
- The reservation's failure modes (remote unreachable mid-create, lease
  lost) leave no half-written issue and say how to retry.

# Notes

Written up from #951 per its acceptance criteria: the choice is recorded
here, implementation belongs with the minibeads tool (this repo consumes it
via ext/minibeads and PATH), so it is filed rather than implemented in agent-utils.
