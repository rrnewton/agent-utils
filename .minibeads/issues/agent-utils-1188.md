---
title: 'state-home-refusal-test-budget: the reclaim state-home refusal test races the 60-second batch lock budget on a busy host'
status: closed
priority: 2
issue_type: bug
assignee: mu-createlock
created_at: 2026-11-10T03:50:00+00:00
updated_at: 2026-11-10T07:08:58.864511450+00:00
closed_at: 2026-11-10T03:53:57+00:00
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

## Resolution

Delivered by mu-createlock (opus 5.5), tests only; the production budget
(`_AGENT_REMOVE_BATCH_ITEM_SECONDS = 60.0`) is unchanged. Root cause, found
while fixing: the test's home user was `sleep 60`. Under gate load the
test's setup and pre-lock checks took more than 60 s, so `sleep` had exited
before the state-home census ran; the census then found no user, remove went
on, and the item budget ran out later (at `git rev-parse`, or at the /proc
scan after the path fence). Remove makes no budget check before the
state-home census, so the budget alone could not pre-empt it. The fix:
the home user is now `sleep 3600`, killed in `finally`; the refusal test runs
remove with `--no-lock-budget`, under a 1-second item budget and a state-home
census slowed by 1.5 s (the host's ~48 s census against 60 s, scaled down),
and still reaches the state-home refusal. A new test,
`test_remove_refuses_when_a_slow_census_spends_the_lock_budget`, keeps the
budget on: the same slowed census spends it, and default remove refuses with
the documented "used its 1-second budget for work before deletion ...
'wrkslots remove --no-lock-budget'" message, keeping the slot and its state
home. The Rust edition's remove has the same state-home gate
(`reclaim_state::prepare_reclaim`) but no item budget, and these tests drive
only the Python edition.

[watermelon-full-preview] 2026-11-09 late ET, mu-gateprio: the sibling test test_remove_reclaims_state_home_and_bundles_unpushed_git in the same file now fails the same way: Refusal "used its 60-second budget" at git rev-parse (pre-deletion checks), at the /proc scan after the path fence, and at git rev-parse --git-path MERGE_HEAD, across one full gate, one exclusive solo, and one plain local pytest on unmodified github/main (49304752) in a nested worktree - pre-existing, independent of #1190. Host load ~52 at the local run. The #1188 fix covered test_remove_refuses_when_a_process_uses_the_state_home; this test drives the same remove-agent-batch budget path and needs the same treatment (test-only budget or --no-lock-budget in the fixture). Evidence: /tmp/gateprio-control-test.log, /tmp/gateprio-rerunP0.log on devbig014.
