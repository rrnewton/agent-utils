---
title: "clean-caches-single-walk: clean-caches walks /proc once per command instead of once per cache"
status: closed
priority: 3
issue_type: feature
created_at: 2026-11-10T10:30:00.000000000+00:00
updated_at: 2026-11-10T18:00:00.000000000+00:00
---

# Description

`wrkslots clean-caches` checks whether a gate is running in each slot (`_gate_running_in_slot`) with a full /proc
walk per cache, so its cost grows with slots times processes. Found while surveying for #1175 census-single-pass,
which left clean-caches out of scope.

# Design

Walk /proc once per command, index the processes by the slot paths they reference (cwd, and the gate admission
records if those are the source of truth), and answer each slot's question with a lookup. Keep the current skip
reasons and the --force-running behaviour.

# Acceptance Criteria

- A walk-count test: one /proc walk per clean-caches run, whatever the number of slots.
- Identical skip and remove decisions to today on a fixture with running and queued gates in some slots.

# Notes

Filed for the lead coordinator (2026-11-10), split out of #1175.

## Resolution

Delivered by cl-fence (opus 5.5). `clean-caches` no longer calls a per-slot `_gate_running_in_slot`, which read the
admission tickets and listed all of /proc once per slot. `_gates_running_in_slots` takes every slot's root at once:
it reads the runs/ and queue/ tickets once (one cwd read per ticket pid), lists /proc once through
`_list_proc_pid_dirs`, reads each process's cwd once and finds the slots containing it by its ancestors, and reads a
command line only when that cwd lies in some slot. Each slot's answer keeps the old precedence: the first ticket in
runs-then-queue order whose pid works inside it, else the first dagrun/validate.py/gate.py/cargo process in listing
order. The listing is skipped when tickets answer every slot or no slot has a root; `--force-running` lists nothing.
Skip reasons, `gate_reason` text, the `--only` refusal and `--force-running` are unchanged.

Tests (test_lifecycle.py): a differential against the pre-#1202 per-slot scan (kept in the test as the reference)
over twelve slots with running and queued tickets, a ticket plus a gate process, dagrun, cargo, two gate processes,
a subdirectory, an idle non-gate child, a `b`/`b-c` prefix decoy, an exited ticket pid, a rootless slot and
malformed tickets: identical verdicts and one proc-root listing (a sabotage letting processes override tickets fails
it). An in-process `clean-caches --yes` over six slots lists /proc exactly once, skips the three gated slots with
today's reasons and removes the rest; `--force-running` lists it zero times. Rust has no clean-caches port (it is
in wrkslotsd's PY_ONLY list), so there is nothing to match there.
