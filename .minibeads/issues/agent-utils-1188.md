---
title: "state-home-refusal-test-budget: the reclaim state-home refusal test races the 60-second batch lock budget on a busy host"
status: open
priority: 2
issue_type: bug
created_at: 2026-11-10T03:50:00.000000000+00:00
updated_at: 2026-11-10T03:50:00.000000000+00:00
---

# Description

`py/wrkslots/tests/test_reclaim_slot_state.py::test_remove_refuses_when_a_process_uses_the_state_home` drives
`remove-agent-batch`, which runs under the default 60-second lock budget. On the gate host a full /proc census
alone takes about 48 s (about 5,000 processes, load near 90). So under full-gate load the batch runs out of budget,
at `git rev-parse` or at "the /proc scan after the path fence" (phase process census), before it reaches the
state-home refusal the test asserts. It failed this way in two #1177 gate runs on 2026-11-09 and passed on a
third solo rerun. In the #1124 gate an hour earlier the shard took 389 s; in the failing runs it took 621 s.

# Design

Make the test check the refusal, not host speed. Run the batch with `--no-lock-budget`, or give tests a
test-only budget (an environment override or a fixture), so a slow census cannot pre-empt the refusal path.
Keep one separate test asserting that the budget still applies when it is set.

# Acceptance Criteria

- The refusal test passes under synthetic load: a census slowed past 60 s still reaches the state-home refusal.
- A test still proves that a configured budget refuses with the documented budget message.
- No change to the production default budget.

# Notes

Found by mu-createlock while gating #1177 abort-create-clone-branches (2026-11-09). Same load-sensitive census
family as #1084/#1102 and the census-under-load failures seen in other gates that night.
