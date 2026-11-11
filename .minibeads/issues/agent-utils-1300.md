---
title: 'team-pid-namespaces: slot liveness from the team namespace instead of host-wide scans'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-11T14:48:05.733093506+00:00
updated_at: 2026-11-11T14:48:14.575375611+00:00
---

# Description

Owner design point (Slack 6231, 2026-11-11): each team (an agent team and its workers) should run in its own PID namespace, so liveness and process scans see only the team's processes. Today slot liveness and removal safety evidence come from host-wide process walks: every scan reads thousands of unrelated processes on a busy host (about 6,700 in the reported case), which is slow, load-sensitive, and the direct cause of census time-bound failures under gate load. Validations already run boxed under cgroup limits, but they share the host PID namespace with every other team, gate, and service on the machine, so a validation's process evidence and a slot's liveness evidence both pay for the whole host.

# Design

Give each team its own PID namespace and stop deriving slot liveness from host-wide walks. Slots launch inside the team namespace (through the existing box: userns/root isolation gains a PID namespace; the cgroup-only mode records why it cannot). Slot liveness comes from the slot's recorded owner and namespace instead: a pidfd on the recorded owner process, cgroup membership of the slot's recorded cgroup, or membership of the team namespace -- never a scan of every host process. Where a census is still required for removal safety, it enumerates only the slot cgroup / team namespace members and states that scope in its evidence. Validations run boxed as today (cgroup CPU/memory/pids limits) plus their own PID namespace per run, so a validation sees only its own tree. Host-wide walks that remain (host diagnostics, audit across teams) stay explicit operator commands, never on the create/remove/liveness hot path. Migration: existing slots keep working (recorded owner pidfd/cgroup evidence already exists); namespace membership is recorded at slot start for new slots and adopted opportunistically for running ones, with the evidence source named in status output so an operator can tell pidfd, cgroup, namespace, and legacy-scan answers apart.

# Acceptance Criteria

- A slot started in a team namespace reports liveness from its recorded owner (pidfd/cgroup/namespace evidence named in status) with no host-wide process walk on the heartbeat, status, or remove path; a test with thousands of unrelated host processes shows census cost independent of host process count.
- Removal safety evidence for a namespaced slot enumerates only its cgroup/namespace members; a process outside the team using the slot path through another mount is still detected by the existing path/mount checks or the refusal names the blind spot.
- A boxed validation run sees only its own PID tree (its init is namespace PID 1 or the documented equivalent) while keeping today's cgroup limits; sibling validations' processes are invisible to it.
- Slots created before namespaces keep today's liveness behaviour until restarted, and status names which evidence source answered for each slot; no slot is judged dead solely because namespace evidence is absent.
- The remaining host-wide scans are listed in the guide as operator diagnostics, each with its scope stated; none runs on create/remove/heartbeat paths.

# Notes

Owner design point Slack 6231 (2026-11-11), filed issue-only on coordinator instruction. Linked to the proc-scan P0 filed separately by its owner lane (the refusal/scan-cost issue): that issue owns the immediate scan bound, this one owns the namespace end state that removes the scan. Census time-bound failures under gate load are the motivating symptom class.
