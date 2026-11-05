---
title: 'browser-latency-load-sensitivity: vibe-talk browser latency budgets still miss under host load after the CPU-budget rework'
status: open
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T18:04:37.530921232+00:00
updated_at: 2026-11-05T18:04:37.530921232+00:00
---

# Description

[muse watermelon-full-preview] Filed from the #827 gate-flake readout on 2026-11-05.

`vibe-talk.check.fold-latency-browser` failed 5 first-run gates in the last 24 h (ledger: 94 first-run gates, 57.4% pass) and `vibe-talk.check.scroll-stability-browser` failed 3, although both pass solo. The fold check (the Shift+O/e main-thread budget) was reworked under the owner's timeout rule in #866, yet it is failing again at host load 80-119, even after #866: a lane saw it fail 4/4 solo runs interleaved at load ~115 and 1/2 on main. The budget is still load-sensitive on main.

# Design

Pick one, with measurement, and record the choice in this issue:

- measure the fold's main-thread/CPU time (performance marks, long-task timing) against a tight CPU budget and relax the wall budget to ~8x, per the owner's #725 rule, if the current check does not already do exactly that; or
- raise the budget to a level a loaded host meets while a real regression still misses it; or
- skip with a named reason when measured host load makes the measurement meaningless, never silently.

Give the scroll-stability walk (a scrollTop-driven walk with no keys, which moved under load) the same treatment. No longer sleeps and no blanket skips: a real main-thread regression must still fail.

# Acceptance Criteria

- Under artificial host load comparable to load 80-119, both checks pass repeatedly (at least 10 consecutive runs) on unchanged code.
- A deliberately introduced main-thread regression of the size the budget exists to catch still fails both checks.
- Solo runs on an idle host behave as before.

# Notes

Named in the #827 readout of 2026-11-05. #866 latency-tests-cpu-budget is closed; this is the remaining load sensitivity it did not remove.

Filed by mu-validate on 2026-11-05 from the #827 gate-flake readout (local number 900); renumbered to 905 on landing by the coordinator, since 900 was taken on main.
