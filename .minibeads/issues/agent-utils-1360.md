---
title: 'mq-close-after-land-budget: closing a landed pull request waits behind other API use for the hourly budget'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-12T13:00:00.000000000+00:00
updated_at: 2026-11-12T13:00:00.000000000+00:00
closed_at: 2026-11-12T13:00:00.000000000+00:00
---

# Description

Closing a pull request after its change lands goes through the same
per-host hourly API budget as every read. On a host whose budget is
small (10 requests/hour) and shared, landed pull requests sat OPEN for
up to 40 minutes: the closer's batched state read and its close calls
were refused as budget-exhausted behind ordinary sync/list use, and each
refusal backed the close off further.

# Design

Give the close-after-landing path its own small budget on top of the
existing hourly cap (pacing, not a hard limit): ordinary calls keep
the whole configured cap, while the closer's batched state read
(`Hosting::pr_states`, used only by the closer) and its close calls
(`Hosting::close`) may spend `github.close_reserve` extra requests per
hour (default 5, `github::DEFAULT_CLOSE_RESERVE`). Bounded: the
hourly total can never exceed the configured cap plus the configured
slice (15/hour on a 10+5 host), so spend stays predictable. One
ledger; no new budget file. (An earlier reserve-from-the-cap design
starved small budgets — a 3-request list on a 6/hour host — and was
replaced.)

Every call, closes included, still goes through the same resolved
`gh` command (`github.gh`, else the `FLEET_REAL_GH` pacing wrapper)
in `Hosting::run_charged`: the slice only changes which budget cap a
charge is checked against, never the channel, so account-wide pacing
still applies to closes exactly as to reads and closes cannot starve
other teams' calls through a side channel.

# Acceptance Criteria

- With cap 10 and reserve 3: ordinary charges stop at 10, close
  charges reach 13, and both refuse past their caps
  (`closes_spend_the_reserved_slice_other_calls_cannot`); ordinary
  calls keep the whole cap even on a 2/hour budget.
- Existing budget behaviour at reserve 0 is unchanged
  (`budget_refuses_past_the_hourly_limit_and_recovers`).

# Notes

## Resolution

[watermelon-full-preview] `Budget` gained `close_reserve` with
`charge_close` capped at `max_per_hour + reserve` and ordinary
`charge` at the configured cap; `Hosting::new` resolves
`github.close_reserve` (0 = default 5); `pr_states` and `close`
charge through the close path. Unit tests above and the label-sync
budget test pass; landing follows the usual validation plus gate.
