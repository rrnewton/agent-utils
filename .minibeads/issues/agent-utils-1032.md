---
title: 'flock-guards-unlock-on-drop: release every flock guard with LOCK_UN, not by closing the file'
status: open
priority: 3
issue_type: bug
assignee: cl-start
created_at: 2026-11-08T00:00:00.000000000+00:00
updated_at: 2026-11-08T15:00:00.000000000+00:00
---

# Description

A flock lock belongs to the open file description. A child forked by ANY thread shares every descriptor until its exec closes them, so a guard that releases by closing its file keeps the lock held through a neighbour's fork->exec window. Its next non-blocking try-lock then sees the lock as busy. The agentctl session_follow test flake had exactly this cause: LifecycleLock and try_record_locks released by close, and a bind found "lifecycle busy" 5% of the time beside a spawning thread. That case is fixed by an explicit-unlock HeldLocks guard (cl-start, branch cl-start/lock-unlock-on-drop).

Other lock guards in agentctl and agent-queue still release by close. They show the problem only when something try-locks without waiting right after a drop (waiting lockers just block for the fork->exec moment), but that pattern is easy to add without noticing.

# Design

- Inventory every flock (and fcntl lock) guard in rs/ (agentctl, agent-queue, others) and in the Python editions.
- Release each with an explicit unlock on drop (the HeldLocks pattern), or use O_CLOEXEC-safe locks that no forked child inherits (open file description locks via F_OFD_SETLK still need the unlock; CLOEXEC alone does not cover the fork->exec window).
- Add a shared helper so new guards get this by default.

# Acceptance Criteria

- No guard releases a lock only by close; a grep-based or clippy-style check enforces it.
- A test per crate keeps a dup of a locked descriptor (as a forked child would) and shows the lock is free after the guard drops.

# Notes

Mechanism and evidence (0 of ~40k try-locks busy with the fix, 5.3% without) are in cl-start's session_follow fix commit, "agentctl: release lifecycle and record locks explicitly, not by closing them".

## 2026-11-08 progress (cl-start)

- First slice gated: 10dfc602 "agent-queue, agentctl: hold flock guards with HeldLock, which unlocks on drop" (agent-queue, agentctl, agent-teams). Still to do: herdr-run (its pane-lock reservation is an Arc<File>; its tests already poll around this), gh-paced, chat-subscription-slack, agent-usage, dagrun, wrkviz (each needs a local guard, having no shared dependency), merge-queue (its owner lane), the Python editions, and an enforcing check.
- The session_follow fix landed as 6cbebc4e (17e83ad1 replayed onto #948 by the coordinator, after rust.agentctl.test, clippy and the differentials passed on the combined tree). Its HeldLocks guard covers LifecycleLock and try_record_locks only; this issue covers the rest.
- Status of the other rust.agentctl.test first-run failures from the gate ledger (2026-11-04 to 11-07):
  - fixed before this survey: tmux_lifecycle "pane %1 not found" (78d8bf87, be66e67c), repoint (9e57a7d1), the unknown_harness batch (#913 e2fec210);
  - not main: launch_spec/restore came from #948's work in progress;
  - taking_a_target_lock_marks_every_lock_file_used: proven and fixed by fd286791 (in the same gated stack as this issue's first slice). The tests in one process share a target-lock root and its refresh marker. A concurrent sender can find the marker this test just removed missing, write a fresh one, and only then scan; the test's own lock_target then skips the refresh, and the test checked before that scan reached its files. Beside a thread taking target locks, the immediate check failed in 48% of rounds; waiting for the files to be marked failed in none, the longest wait 2 ms.
  - chat_service idle backstop (8.14 s against the 8 s bound, once, 2026-11-04 01:10Z, normally 5.50 s): already fixed by #869 (d23e7c2c), no code change. That run kept its test state on disk-backed /tmp (its log names /tmp/agentctl-managed-1920065-33), before #869 moved the agentctl tests to a memory-backed scratch directory five hours later. The reply store is fsync'd, so a ~2.6 s disk stall explains it. CPU starvation does not: under CPUQuota 200% down to 25% with the whole lib suite on 4 threads, the wait stayed 5.50-5.69 s. There was no recurrence in the 244 gates after #869.

