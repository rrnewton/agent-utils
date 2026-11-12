---
title: 'gate-hold-covers-local-preflight: an admission hold that stops gates before their preflight'
status: closed
priority: 1
issue_type: task
assignee: mu-gatehold
created_at: 2026-11-12T13:31:00+00:00
updated_at: 2026-11-12T13:31:00+00:00
---

# Description

On 2026-11-12 the coordinator held gate admissions from 7:26 to 8:01 AM
ET (an exclusive placeholder run in the gate admission dir, via the
harness gate-hold script) so another team could take a CPU-measured
reading. The hold blocked queue tickets, but gate.py's local preflight
(the selected checks a lane runs before it takes a ticket) still ran
dagrun scopes at 7:43-7:54, uncontained. A hold that only blocks tickets
does not keep the host quiet, and an uncontained preflight can itself
disturb the measurement.

# Design

- A first-class admission hold in gate.py: `gate.py hold --until <time>
  --reason <text>` / `gate.py hold --release`, stored as hold.json in the
  admission dir. While a hold is active, a new gate.py invocation waits
  before its local preflight (or exits 79 with --no-wait-hold, naming the
  reason and end time; --wait-timeout bounds the wait at 75). Running
  gates and a nested phase of a live parent are untouched; an expired
  hold is reaped and ignored. `gate.py status` shows the hold.
- The local preflight runs contained like a gate: the same per-gate CPU
  cap (default 48 CPUs) and nice 10, via the existing containment path,
  with VALIDATE_MAX_CPUS/GOMAXPROCS capped the same way.

# Acceptance Criteria

- Tests without wall-clock budgets (wait on conditions): a hold blocks
  preflight, release unblocks, an expired hold is ignored, preflight
  containment flags are applied, and status shows the hold.
- The harness gate-hold script is not edited here; the handoff tells the
  coordinator how to switch to the new command.

# Notes

Number reserved in the host ledger; filed and landed by the
mu-gatehold lane. [watermelon-full-preview]
