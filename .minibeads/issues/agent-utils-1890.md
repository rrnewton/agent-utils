---
title: 'agentctl-turnover-watch-health-window: scale the watch health window and continue past rollbacks'
status: closed
priority: 1
issue_type: task
labels:
- agentctl
created_at: 2026-11-19T01:37:09-05:00
updated_at: 2026-11-19T16:02:19-05:00
---

# Description

[watermelon-full-preview] Turnover's health check gives a kind=watch
unit only 120 s for its first health pass, while watch-agents gets
pass-budget + 90 s. On a large registry the first pass takes longer,
so turnover rolls back a healthy new process and stops the whole run:
one host's 05:30 ET turnover failed on its repair-watch exactly that
way, leaving several units on the old snapshot.

# Design

In `services refresh`'s health timeout: a watch daemon's window scales
from its configured pass budget or interval exactly like watch-agents
(budget + 60 s + 30 s soak), and an explicit per-unit timeout
(`--health-timeout-for UNIT=SECONDS` on turnover and refresh, held
only in the options structs — no new persisted field) wins exactly.
Turnover continues past a unit whose rollback completed, reports the
rolled-back units at the end (text and a `rolled_back` field), and
still stops for anything harder; standalone refresh keeps its
stop-on-failure order.

# Acceptance Criteria

- A slow first watch pass inside the scaled window refreshes instead
  of rolling back; an explicit per-unit timeout is honoured exactly.
- A turnover whose one unit rolls back still moves the remaining
  units and reports the rollback at the end, with a test using a slow
  first pass.

# Notes

[watermelon-full-preview] Filed from coordb's brief after the 05:30 ET
failure. Services/turnover is Rust-only (the Python edition refuses
`services`), so there is no Python mirror or differential group.

## Resolution

[watermelon-full-preview] Landed on main as
871a07ff4ee1b1d94bc729c0a022c5e7b79b4fa0, rebased over #1900
(cde2597f) keeping both behaviours: #1900's scaled window and
pass-progress proof, plus this issue's exact per-unit
`--health-timeout-for UNIT=SECONDS` override and turnover continuing
past a completed rollback (rolled-back units reported at the end in
text and the `rolled_back` field; standalone refresh unchanged).
No new persisted field. Tests: slow first watch pass, explicit
per-unit override, continue-past-rollback, and the full services
suite (58) on the rebased tree; gate passed with no overlap flags.
