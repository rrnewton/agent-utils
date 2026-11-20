---
title: 'agentctl-pidfd-open-einval: pidfd_open EINVAL on a new kernel must fail closed, never read as death'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- robustness
created_at: 2026-11-20T02:09:21-05:00
updated_at: 2026-11-20T02:09:21-05:00
---

# Description

[watermelon-full-preview] On the reprovisioned host (kernel 6.13), `os.pidfd_open` raises `OSError EINVAL` in `py/agentctl/foreign/ownership.py:245`, where `_open_pidfd` handles only `InterruptedError` and `ProcessLookupError`. Four to five tests fail identically on `github/main` (`python.agentctl` shard-0/1): `test_crashed_runner_owned_harness_is_cleaned_before_state_reuse[down|gc|restart]` and `test_another_process_cleans_a_crashed_runners_tree_by_proof_only`. The exception escapes `pin()`, so cleanup of a crashed runner's harness tree crashes instead of proving or refusing.

Why EINVAL happens is not yet known: candidates are a thread id passed instead of a thread-group id, an unsupported flag (this call site passes none), or a pid in another namespace. The diagnosis must come first, from the failing call path, not from guessing.

# Design

- Reproduce the EINVAL on the failing tests and identify which pid reaches `os.pidfd_open` and why the kernel refuses it (thread id vs tgid, namespace, or other); record the finding in this issue's resolution.
- Handle it correctly and fail closed: an unprovable identity is never "dead". EINVAL from `pidfd_open` must yield the unproven verdict (no signal, no state reuse), never the gone verdict; only positive evidence (no such process, newer generation, exited state, changed boot) may certify death, as `pin()` documents today.
- Check the Rust edition's pidfd use (`rs/agentctl/src/foreign/ownership.rs` and callers) for the same issue and apply the same fail-closed handling there if present.
- Tests drive the exact defect: the four named tests pass, plus a case where EINVAL is forced (or the diagnosed trigger is reproduced) and the outcome is unproven, not gone. Condition-based waits only (owner rule 346); tests pass on a host above 75% CPU.

# Acceptance Criteria

- The root cause of the EINVAL is identified and recorded (which pid, which kernel rule).
- `test_crashed_runner_owned_harness_is_cleaned_before_state_reuse[down|gc|restart]` and `test_another_process_cleans_a_crashed_runners_tree_by_proof_only` pass on the new kernel; the wider `python.agentctl` foreign/ownership suites stay green.
- An unprovable pidfd identity never authorizes a signal or a death certificate, in either edition; the Rust edition is checked and, if affected, fixed the same way with its tests green.
- No persisted field changes (reader-skew check: none expected; report any new persisted field before gating).

# Notes

New-kernel robustness, reported 2026-11-19 after the host reprovision (kernel 6.13). Lane mu-agentctlfix, nested worktree `.muse/worktrees/agentctlfix` on branch `mu-agentctlfix/1936`. Companion: #1937 agentctl-status-redact-argv, same lane, separate commit. Browser checks failing for a missing system library are unrelated and tracked separately.
