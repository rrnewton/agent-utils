---
title: 'agentctl-revive-host-process-isolation: revive tests refuse on unrelated same-uid host processes'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
- robustness
created_at: 2026-11-21T03:02:08-05:00
updated_at: 2026-11-21T03:02:08-05:00
---

# Description

[watermelon-full-preview] Four `subagents::tests::revive` tests failed
in the #2027 gate's `rust.agentctl.test` step on a loaded host, all with
the same refusal (pid varies): "cannot inspect environ of same-uid
process 1896290 while proving no live holder of the recorded session;
refusing (fail closed) — remedy: retry once that process exits or is
inspectable". The tests:

- `subagents::tests::revive::five_stop_and_revive_cycles_complete_for_every_harness_that_revives`
- `subagents::tests::revive::revive_dry_run_with_a_gone_pane_never_proves_unrelated_shells`
- `subagents::tests::revive::revive_live_session_holder_refuses`
- `subagents::tests::revive::revive_heals_a_slot_project_recorded_as_the_slot_itself`

The holder proof scans real host processes, so an unrelated same-uid
process whose environ cannot be inspected at that moment blocks the
proof and fails the test. That is a test-isolation defect (tests must
not depend on real host processes), not only load flakiness: the same
tests passed in the coordinator-requested solo rerun of
`rust.agentctl.test` (2935 passed, 0 failed) once the host processes
were gone or inspectable.

# Design

- Give the revive holder proof a seam so tests enumerate only the
  processes they started (or a scripted process table), never the real
  host's same-uid population.
- Keep the production fail-closed rule unchanged: an uninspectable
  possible holder still refuses a revive.
- Show the four tests green in a full `rust.agentctl.test` gate step on
  a host with unrelated same-uid processes running.

# Acceptance Criteria

- The four named tests pass regardless of unrelated same-uid host
  processes (present, exited mid-scan, or uninspectable).
- Production revive still refuses when a possible live holder cannot
  be inspected.
- No assertion is weakened: each test still proves the behaviour its
  name states.

# Notes

[watermelon-full-preview] Filed by lane mu-pidfdenoent at the
coordinator's direction during #2027 agentctl-pidfd-open-enoent-thread;
number 2037 assigned by the coordinator. Related: #2031
agentctl-lib-load-flakes (same gate, different wait). No lane, host or
person is named here.

## Resolution

[watermelon-full-preview] Landed as 041006a7 on main. A
test-only thread-local fake process table replaces the holder
scan's host enumeration in the four named revive tests plus
dead_native_retirement_previous_boot_missing_pane_stale_agent_archives
(the retire path reads the same scan; the gate showed the same
case there). The holder is scripted as a process row and the
others install an empty table; no assertion was weakened and the
production /proc scan is unchanged.
