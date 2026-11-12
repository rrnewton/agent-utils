---
title: 'agentctl-lock-rank-load-panic: a lock_rank test in rust.agentctl.test panicked under host load and passed alone'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- validation
created_at: 2026-11-12T11:05:29+00:00
updated_at: 2026-11-12T11:05:29+00:00
---

# Description

[opus 5.5] On 2026-11-11 a full gate for a page-only change (#1321 after-done-setting) went red in `rust.agentctl.test` with a panic in the `agentctl::subagents::tests::lock_rank` module, on a host under heavy load. It passed on a solo `gate.py --rerun` of that step. The change touched no agentctl code, so the panic was in main's code, not the change's.

The lock_rank tests exercise lifecycle-lock contention: a priority manager taking a contended lock first, sends giving up at their deadline while an agent stays locked, sends waiting for readiness holding no lock, and read-backs. A panic there under load could be a real concurrency bug (lock ordering, or a waiter seeing a lock released out of rank order) rather than only a timing-sensitive test. Under owner rule 346, either is a defect: gating tests must pass with the host above 75% CPU.

Evidence gap: the gate ledger entry has no log path, and the lane's slot was removed after landing, so the exact test name and panic message weren't kept. The first step is to reproduce it.

# Design

- Reproduce under load: run the lock_rank tests repeatedly inside a saturated CPU-quota scope (the #1250 proof bar's setup), only while the host is quiet (no other team's merge-queue validation active).
- Classify the panic: a lock-rank or ordering assertion is a product bug, fixed in the lock code; a wall-clock deadline or sleep in the test is a test bug, fixed by waiting on events or conditions with generous timeouts (at least 5x solo).
- If it can't be fixed quickly, quarantine the failing test under this issue, as #1250 does.

# Acceptance Criteria

- The panicking test and its message are recorded in Notes.
- The lock_rank module passes 20 of 20 in a saturated scope, and a deliberately broken lock-ordering variant still fails.
- No wall-clock budget decides any lock_rank verdict.

# Notes

Numbered from devbig030's interim block (1400-1469). The devbig014 lead asked for it to be filed (2026-11-12).
