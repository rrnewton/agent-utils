---
title: 'wrkslots-owner-live-stale: a slot row reads owner=live 44 hours after its heartbeat expired, with no registered agent of that name'
status: open
priority: 2
issue_type: bug
assignee: cl-wrkdoctor
labels:
- wrkslots
- liveness
created_at: 2026-11-28T09:36:00-05:00
updated_at: 2026-11-28T11:00:00-05:00
---

# Description

Reported by a consuming team's coordinator on 2026-11-28: on that team's second host, a slot row's status reads `owner=live` although the row's heartbeat expired about 44 hours earlier and the agent registry has no agent of the recorded name. Either the owner really is alive (a process that still matches the recorded owner identity) or wrkslots' owner-liveness verdict is wrong for this row.

Open questions:

- How does wrkslots decide `live` for a recorded owner (the owner process-state check over the recorded identity)? Does the recorded identity carry a process start time or boot id that defeats PID reuse, or a bare PID that a later, unrelated process can satisfy?
- Does heartbeat age or TTL expiry enter the owner verdict at all, or only the diagnosis text? (The user guide says heartbeat age is diagnosis only and never authorizes removal.)
- Direction of risk: can a false `live` block a legitimate reclaim (a stale slot that cannot be removed), and can any false verdict allow a wrong reclaim (removing a slot whose owner really is alive)?

# Design

1. Read-only analysis first, both editions: trace the owner verdict from the recorded owner identity to the rendered `owner=` field (Python CLI and the Rust readers), with file:line, and name every input to it (PID, process start time, boot id, executable or cgroup evidence, heartbeat age).
2. Reproduce on a scratch project only (`WRKSLOTS_INIT_REPRESENTATION=worktree`): record an owner, let it exit, expire the heartbeat, and check what status reports; then put a different live process at the same PID if the identity allows it (or show why it cannot), and check the verdict after a simulated reboot (boot id change).
3. Fix only what the analysis shows is wrong: if a reused PID or a changed boot can read as the recorded owner, compare the full recorded identity; if the verdict is right but the display is misleading, show the evidence (which process matched, its start time) and the heartbeat age next to `owner=live`.

# Acceptance Criteria

- The owner-liveness decision is documented with file:line for both editions, including whether PID reuse and reboots are defended against and whether heartbeat age is an input.
- The direction of risk is stated: what a false `live` can block, and whether any verdict can allow a wrong reclaim.
- A scratch-project reproduction (or a proof it cannot happen) of a `live` owner with an expired heartbeat and no registered agent.
- If a defect is found: a test that a different process at the recorded PID, or a different boot, does not read as the recorded owner, in both editions where the surface exists.

# Notes

Report evidence at filing time; this lane has not inspected the reporting host and will not (reproduction on a scratch project only). Number pre-assigned by the coordinator (2026-11-28).

## Analysis (cl-wrkdoctor, 2026-11-28)

- Python: `_status_record` takes the owner verdict from `_process_state_with_evidence` and `_process_state_legacy` over `ProcessIdentity` (host PID, start ticks, boot id, host id; cgroup and namespace fields are evidence only). A changed boot reads dead; a PID that names no live process reads dead; the same PID with other start ticks reads "reused" and dead; a zombie reads dead. Live only when /proc/PID now carries the recorded start ticks in the recorded boot. Rust: `status_state::process_state`, the same comparison.
- PID reuse is defended: a reused PID would need the same start tick in the same boot. Heartbeat age is not an input: `_heartbeat_diagnosis` feeds only `heartbeat_expired` and `heartbeat_age_seconds`, which the status row prints beside `owner=`.
- Scratch reproduction (worktree representation): an owner process the creator spawned and left running, an agent name no registry knows, a 60 s time-to-live: status reads owner=live ("PID N generation is live", pidfd evidence), liveness unverifiable, heartbeat expired. So the reported row most likely names a recorded owner process that is still running: a leftover of the old lane, or a long-lived process passed as the owner.
- Direction of risk: a live owner can only block a reclaim (removal needs a dead owner generation unless an owner release applies; an attested stop also needs the owner generation proven dead). "Live" never authorizes a removal, and a false "dead" needs other start ticks or a new boot. No wrong-reclaim path found.

## Fix (display only, the consumer lead's choice)

Status rows name the live process (`owner_process`: pid, start instant, command name, truncated command line, cgroup) when the owner is live and the heartbeat expired; doctor reports `owner-live-heartbeat-expired`, keyed by the slot, only past TTL + max(TTL, 1 hour) since the recorded heartbeat, so one long turn reports nothing. Both editions; consumers that alert on new finding kinds get one stable key per stuck slot.
