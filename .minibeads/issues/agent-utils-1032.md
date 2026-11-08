---
title: 'flock-guards-unlock-on-drop: release every flock guard with LOCK_UN, not by closing the file'
status: open
priority: 3
issue_type: bug
assignee: cl-start
created_at: 2026-11-08T00:00:00.000000000+00:00
updated_at: 2026-11-08T04:00:00.000000000+00:00
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

- The session_follow fix landed as 6cbebc4e (17e83ad1 replayed onto #948 by the coordinator, after rust.agentctl.test, clippy and the differentials passed on the combined tree). Its HeldLocks guard covers LifecycleLock and try_record_locks only; this issue covers the rest.
- Status of the other rust.agentctl.test first-run failures from the gate ledger (2026-11-04 to 11-07):
  - fixed before this survey: tmux_lifecycle "pane %1 not found" (78d8bf87, be66e67c), repoint (9e57a7d1), the unknown_harness batch (#913 e2fec210);
  - not main: launch_spec/restore came from #948's work in progress;
  - open: taking_a_target_lock_marks_every_lock_file_used (probable cause: the tests in one process share a target-lock root and its refresh marker, so a concurrent sender's in-flight refresh can make this test skip its own) and the idle-backstop wall-clock bound in chat_service (8.14 s against 8 s once, under load).

