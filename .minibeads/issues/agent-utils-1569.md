---
title: 'eden-boxed-lanes-audit: what works for Eden slots and boxed lanes under each isolation mode'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- agentctl
created_at: 2026-11-15T20:30:00+00:00
updated_at: 2026-11-15T20:30:00+00:00
---

# Description

[watermelon-full-preview] Owner directive: subagents run
mount-sandboxed with allow-listed write dirs (`wrkslots run`/`box`,
`slot_isolation`); lanes on the reference host were running with
isolation unset. Audit what works today for an Eden slot under each
isolation mode (userns, root, cgroup): the FUSE mount, the edenfs
daemon socket, `sl`, and buck2; widened 2026-11-15 to agentctl
start/readiness/send/stop for every harness (Claude, Codex, Muse)
boxed the same way. The full support table, probe transcripts and
gap list live in the harness repository report
`ai_docs/reports/research/2026-11-15-eden-boxed-lanes.md`.

Headline results (live-probed 2026-11-15 on throwaway slots/lanes):
no harness works mount-sandboxed out of the box — all three die at
startup in a userns box with `Error setting effective UID to root:
EINVAL` (fixed for Muse by the adapter boxed launch under #1583);
cgroup completes the full lane cycle; boxed Eden entry itself was
refused by the Git sibling rule on the backing path (fixed, see
Resolution progress in Notes); `sl` against a host backing needs a
writable store lock, so boxed `sl` stays broken until the in-image
backing of #1567 can be bound writable.

# Design

Audit-first: code audit plus read-only/throwaway live probes, a
support table per isolation mode, and each gap named with a
proposed fix. Fixes land as their own commits/issues (Eden box
entry here; harness launch and retire under #1583; the fixed-alias
alias under #1685), never bundled silently into the audit.

# Acceptance Criteria

- Support table published with per-mode FUSE/socket/sl/buck2 and
  per-harness start/send/read/stop cells, each observed or marked
  needs-probe with the exact probe to run.
- Gaps filed or fixed with coordinator agreement (#1583, #1685).
- Final probe rerun on a throwaway Eden slot with the landed fixes
  (entry, checkout read/write, socket, other-slot isolation).

# Notes

Progress: Eden box-entry fix landed (slot box accepts Eden slots;
backing deliberately not bound; image convert refuses Eden). The
final probe rerun waits on the host Eden freeze window and the
landed boxed-launch fix. Resolution: (fill in at landing)
