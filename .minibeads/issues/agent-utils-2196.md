---
title: 'wrkslots-persistent-lane-box-enter: enter a persistent per-lane box per shell command instead of building the view per command'
status: open
priority: 1
issue_type: feature
labels:
- wrkslots
- agentctl
- sandbox
assignee: mu-boxenter
created_at: 2026-11-24T06:20:00-05:00
updated_at: 2026-11-24T06:20:00-05:00
---

# Description

#2192 agentctl-harness-shell-tool-sandbox proved the split (harness outside the box, only its shell tool wrapped into the slot view) confines shell and edit writes for Claude Code and Codex under userns, but a fresh `wrkslots run --isolation userns` per command costs a median 5.7 s wall / 4.5 s CPU over N=20 against about 2 ms direct, because every call rebuilds the whole file-system view. That is unshippable for an interactive agent. This issue builds the persistent view: one long-lived box per lane, built once, entered per command.

# Design

One keeper process per lane holds the built view (user and mount namespaces plus the slot's cgroup placement) for the lane's lifetime; it is owned by the lane and cleaned up with it. The harness hook rewrites each shell command to enter that existing box with setns into the keeper's user and mount namespaces (joining the keeper's cgroup first) instead of building a view. Entry verifies the keeper through a pidfd and the recorded namespace identities and process start time, and re-checks its own namespaces after entry, so a stale or recycled pid is never entered and a command never runs with the host view or outside the box's cgroup. While no live keeper exists the entry refuses with a clear message (fail closed, never an unboxed fallback); an explicit ensure rebuilds a dead keeper. A keeper that dies mid-command does not kill the entered command, whose namespaces and cgroup membership it already holds. userns mode lands now; root mode follows through wrkslots' own helper invocation once the root-isolation box work reports working. The wrkslots side gains the keeper lifecycle and the fast entry; the agentctl side gains the opt-in profile options designed under #2192 and generates the per-harness hook configuration (Claude Code and Codex, including the Codex exec apply_patch matcher; Muse has no tool hook and stays unsupported). Per-command target: well under 0.2 s median and p90 over 20 runs, CPU and wall.

# Acceptance Criteria

- On a throwaway test lane in a test registry (never a live lane): a write outside the slot fails from the shell tool and from the edit tool, for Claude Code and for Codex (whose edits arrive as an exec apply_patch).
- cargo build and pytest of a small project inside the slot work through the entered box.
- Per-command overhead of entering the persistent box is well under 0.2 s at median and p90 over 20 runs, CPU and wall, reported beside the #2192 per-call numbers.
- A killed keeper is rebuilt by ensure, the entry refuses with a clear message while no keeper exists, and a keeper killed mid-command leaves the running command confined and able to finish.
- A stale or recycled keeper pid is never entered (pidfd or namespace-identity check), and an entered command provably holds the box's namespaces and cgroup, never the host's.
- The keeper is owned by the lane and removed with it; no live profiles change, per-test config directories only, strict-MCP empty config for the test lane, and the harness support matrix moves in the same commit as any agentctl code that changes a cell.

# Notes

[watermelon-full-preview] mu-boxenter. Number pre-assigned by the coordinator. Owner direction via the lead coordinator: agents start normally but their shell tool always runs inside a sandboxed file-system view. Prior work: #2192 agentctl-harness-shell-tool-sandbox (the per-call prototype and its measured blocker) and the harness support matrix Boxed launch row. Root mode waits on the root-isolation box signal through the coordinator; no direct sudo. Muse waits on a tool hook from its own team and is out of scope here.
