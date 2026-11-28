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
updated_at: 2026-11-28T09:36:00-05:00
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
