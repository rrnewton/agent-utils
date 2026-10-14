---
title: 'host-admission: decide whether to land the shared host resource-admission core'
status: open
priority: 4
issue_type: feature
labels:
- dagrun
- needs-owner
created_at: 2026-10-14T14:48:59.071865569+00:00
updated_at: 2026-10-14T14:48:59.071865569+00:00
---

# Description

[opus 5.5] Found by the 2026-10-14 branch triage (ai_docs/transient/2026-10-14-branch-triage/REPORT.md). Commit 88c76cfb, kept on archived/rescue/<host>/codex-host-admission-phase1-20260924-88c76cfb (host name redacted here), holds a shadow-only shared host resource-admission library in both editions (py/host_admission and rs/host-admission, about 6.7k lines): a versioned flock-protected ledger, GRANT/QUEUE/REFUSE/UNKNOWN outcomes, priority FIFO tickets, atomic memory plus named-token leases, owners bound to host, boot, PID and start time, shared fixtures, and a cross-edition differential. Nothing like it is on main; dagrun's run --admission gate is narrower (memory only, per runner). As written it changes nothing at runtime, because its dagrun adapters are never called, yet it makes dagrun depend on it, and its intended consumers live outside this repository. Owner decision first: is a shared host-admission authority still wanted? If yes: rebase the four commits; register both packages in validation routing, components and the package checks; wire its differential into cross/; add RELATED_WORK.md and docs; and either wire read-only shadow sampling into dagrun run --admission or drop the dependency.

# Acceptance Criteria

The owner's decision is recorded. If landing: both editions are packaged and validated with a passing differential, and dagrun gains no unused dependency.
