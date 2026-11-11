---
title: 'wrkslots-audit-rlimit-leak: a read-only scan leaves its RLIMIT_AS cap on the calling process'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T05:58:47+00:00
updated_at: 2026-11-11T09:04:17+00:00
closed_at: 2026-11-11T09:04:17+00:00
---

# Description

[watermelon-full-preview] Follow-up to #1247/#1261, reported by a consumer lane. `_cmd_audit` (and the other read-only handlers) applied the 4 GiB `--scan-memory-limit` RLIMIT_AS cap to their own process and never restored it. A CLI invocation exits anyway, but an in-process call (pytest, an embedding) leaves the worker at a 4 GiB soft limit for everything that runs after it in that process, which can fail later allocations that would otherwise succeed.

# Design

`_apply_scan_memory_limit` returns a restore callable holding the previous soft limit; each read-only entry point (status, doctor, audit, unpushed) runs it on every exit path (return or raise). The cap itself is unchanged.

# Acceptance Criteria

- A test calls audit in-process and asserts the process RLIMIT_AS is byte-identical afterwards.
- The existing cap-application test still pins the applied limit and its restore.

## Resolution

Landed on main as 658948c1 (mu-statuslock). Gate 0d708f1c ran the full contract on f53bed51: every red was in the host-load census/liveness/reclaim class (#1267); the four unlisted remove/ownerless differentials pass solo, and the new in-process limit test passes. No changed-path regression.
