---
title: 'restore-drill-notices-missing: the restore drill finds no brief or catch-up notice queued for a revived lane on one host, even uncapped and on clean main'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-13T02:11:52+00:00
updated_at: 2026-11-13T02:11:52+00:00
---

# Description

[opus 5.5] `tests/test_agentctl_restore_drill.py::test_restore_drill_happy_path_fresh_pane_slot_and_rerun` fails on one of the two development hosts at line 747:

    assert drill.queued(name) == [f"restore-brief-{boot8}-{name}.json", f"restore-catchup-{boot8}-{name}.json"], name
    AssertionError: cheap   (got [])

After a complete restore that revived every row, the drill finds NO brief and NO catch-up notice queued for the revived lane `cheap`. It fails the same way:
- in contained gates (8-CPU quota) for two unrelated changes (#1359 and a clean checkout of main at 8500c1ac, without #1359);
- in a solo rerun under the same cap;
- in a solo UNCAPPED rerun on a quiet host with no other runner active (2026-11-12, 1299 s for the shard).

The same test is green on the other host at e5fc567c, which already contains #1405 restore-epoch-manager-restart (c3f64eb5) and #1372 chat-stale-prompt-wedges-inbox (0d15e1e2, which added the restore-notice supersede keys restore-brief/restore-catchup/restore-imback).

So the earlier explanation (fails only under a CPU-capped scope) doesn't hold. This may be a REAL restore defect: a real outage on that host could revive lanes without their brief and catch-up notices, which the team-restore protocol relies on to tell lanes what happened.

# Design

Diagnose first, time-boxed to about 2 h, report before any fix. Leads to check, in order:
1. Isolation: does the drill really use only a scratch registry and a scratch terminal multiplexer, or can a live watcher, chat bridge, or restore/watch unit on the host see and consume the scratch registry's queue items, or the host's real agentctl state?
2. The restore epoch from #1405: on this host the user-manager identity comes from `user@UID.service` InvocationID. Check whether the epoch match makes the drill's rerun skip the notice step, or makes the first run decide the notices were already sent.
3. The supersede keys from 0d15e1e2: check whether a supersede or expiry drops the brief and catch-up before the assertion reads the queue (for example, a self-expiring notice with a wall-clock lifetime shorter than the drill's slow first run on a busy host).
Compare a run on each host with the same commit if possible. Record the queue directory contents and the notice lifecycle (written, superseded, delivered, archived) at each drill step.

# Acceptance Criteria

- A diagnosis report naming the cause, with evidence from both hosts.
- If it's a defect: a fix with a regression test that fails before the fix and passes after, with no wall-clock budget (owner reply-346 rule).
- If it's test isolation: the drill isolated from live host state, with a test showing it.
- The drill green on both hosts, or quarantined with this issue as its tracking issue until it is.

# Notes

Number reserved by the other host's coordinator, filed from devbig030's side at its request (2026-11-12). Supersedes the narrower "restore-drill-happy-path-cpu-capped" description.

## Resolution

[watermelon-full-preview] Test-side only; no restore defect. After
#1372 (`0d15e1e2`) the resume brief and catch-up carry supersede keys,
which route them to the queue's `inbox.v2/` lane; the drill's
`queued()` read only the classic `inbox/`, so it saw `[]` on both hosts
since #1372 landed (the devbig014-green premise was withdrawn by the
lead). Full diagnosis, with the instrumented queue dump and the
real-restore delivery path (the Rust drain merges both lanes), in
https://github.com/rrnewton/dev-agent-utils/blob/main/ai_docs/reports/research/2026-11-12-restore-drill-notices-missing.md
Fixed by making the drill read both queue lanes, resolving the brief
across lanes, and pinning `HOME` to the scratch root; downgraded
P1 to P2 as a drill-maintenance bug.

Gate (contained, base `b73d21634f`, 4495.3s): 54 passed, 1 failed step.
The restore drill passed inside `python.agentctl.shard-1`, and both
cross differentials passed with zero divergence
(`cross.agentctl.herdr-agent-differential`: 104 paired checks agree;
`cross.agentctl.differential-2`: 357 paired checks agree). The only
red was shard-1's two chat-launch kill-timing tests, waived by the lead
as known untouched load-sensitive defects (this change touches only
the drill; it does not touch chat launch):

```text
[python.agentctl.shard-1] FAILED tests/test_agentctl_chat_launch_rust.py::test_launch_sigterm_exits_within_a_few_seconds
[python.agentctl.shard-1] FAILED tests/test_agentctl_chat_launch_rust.py::test_killing_outer_pytest_leaves_no_chat_run
dagrun: FAIL - 54 passed, 1 failed, 0 aborted, 0 intentionally skipped, 0 runtime-skipped, 0 dependency-skipped, 0 not launched in 4495.3s
```

Standalone contained runs of the drill file: 4 passed in 1647.46s
(before the rebase) and 4 passed in 1718.41s (after it).
