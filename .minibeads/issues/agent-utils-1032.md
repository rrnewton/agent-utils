---
title: 'flock-guards-unlock-on-drop: release every flock guard with LOCK_UN, not by closing the file'
status: open
priority: 3
issue_type: bug
assignee: cl-start
created_at: 2026-11-08T00:00:00.000000000+00:00
updated_at: 2026-11-08T00:00:00.000000000+00:00
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
