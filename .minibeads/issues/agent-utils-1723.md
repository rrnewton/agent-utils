---
title: 'agentctl-revive-binding-lock-self-deadlock: revive blocks on its own queue binding lock'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-16T16:35:52+00:00
updated_at: 2026-11-16T18:55:00+00:00
closed_at: 2026-11-16T18:55:00+00:00
---

# Description

[watermelon-full-preview] Evidence (2026-11-16 11:35 AM ET, from a gate rerun on an unrelated change): `rs/agentctl/tests/tmux_lifecycle.rs` test `tmux_backend_runs_the_full_agent_lifecycle` hung for 25+ minutes. Its child process `agentctl --registry <test registry> --term-backend tmux --tmux-socket agentctl-tmux-test-<pid>-0 revive worker --startup-timeout 30` was blocked in the kernel (wchan `locks_lock_inode_wait`). `lslocks` for that PID showed FLOCK WRITE held on: `registry/agents/worker/queue/.delivery.lock`, `registry/agents/worker/queue/.binding.lock` (listed TWICE: two open file descriptions on the same file), `registry/locks/worker.lock`, `registry/.identity.lock`. `flock()` locks belong to the open file description, so taking `.binding.lock` through a second `open()` while the first is held blocks the process on itself. The same test passed in other runs, so the path is conditional (likely revive with a queue/binding state left by an earlier step; maybe a retry path). Revive is how lanes come back after an outage, so a self-deadlock here defeats self-healing.

# Acceptance Criteria

- Every place revive (and related paths: restore, restart, delivery, rebind) acquires `queue/.binding.lock` (and the other locks above) is found, with the call path that re-opens it while held.
- Fix: one acquisition per process per lock, passed down (a guard), or a re-entrant wrapper that refuses a nested open; plus a documented lock order if two paths can take these locks in different orders.
- A deterministic regression test reproduces the nested acquisition (not timing-based: drive the exact state, assert revive completes, with a generous timeout only as a backstop).
- The Python edition (`py/agentctl`) is checked for the same pattern and fixed or filed.

# Notes

## Resolution

[watermelon-full-preview] Fixed by making queue-lock acquisition re-entrant per thread. `agent-queue`'s `fsio` now keeps a registry of the locks the calling thread holds (by thread+device+inode, depth-counted): `HeldLock::new` registers, `HeldLock::reentrant` holds one more guard without unlocking on drop, `already_held` lets the queue `lock_exclusive` return at once, and `lock_exclusive_blocking` is the blocking acquisition lifecycle paths use. Rust `queue_locks`, `queue_locks_at`, retirement queue locks and `identity_lock` all go through it, so revive (which holds lifecycle + identity + queue delivery/binding locks across launch, finish, carry and drain) can never wait on itself whatever queue path it calls back into. The lifecycle (ranked) lock is deliberately NOT re-entrant: distinct operations exclude each other through it even in one thread, and revive drops it before draining. The lock order (lifecycle, identity, queue delivery, queue binding, pane) is documented in agent-queue's crate docs. The Python edition mirrors this with `_acquire_flock`/`_release_flock` in `agent.py` (registry keyed by thread; guard never held across the flock itself), used by queue bind/rebind, serialized enqueue, and `subagents`' lifecycle, identity and queue locks; its drain keeps its single-descriptor unlock/relock cycle (it never re-opens, and revive drains only after its locks are released). Regression tests: `fsio::tests::a_lock_this_process_holds_is_never_taken_again`, `revive_completes_while_this_process_already_holds_the_queue_locks` and `revive_archived_completes_while_this_process_already_holds_the_queue_locks` (Rust, the second covers the `--archived` restore path), and `test_queue_binding_completes_while_this_process_holds_the_queue_locks` (Python) — each drives the exact held-lock state and asserts completion, with only the harness timeout as a backstop. Note: `revive --dry-run` takes no locks at all (it returns its plan before any lock acquisition), so a dry-run hang has a different cause than this self-deadlock (filed separately as #1737). Landed on main; see the landing commit for the SHA.
