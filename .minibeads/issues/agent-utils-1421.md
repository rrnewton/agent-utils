---
title: 'gate-resource-profile: a host-file resource profile for gates (nice, IO class, MemoryMax) beside the CPU quota'
status: open
priority: 2
issue_type: feature
labels:
- validation
created_at: 2026-11-12T15:53:57+00:00
updated_at: 2026-11-12T15:53:57+00:00
---

# Description

[opus 5.5] On a host that donates CPU to another team's measured work, that team's lead allows our gates to run during their windows only if the gate is unpinned, runs at nice 19 with idle IO, and sits in a cgroup with CPUQuota at most 800% plus a MemoryMax (2026-11-12). gate.py can set only the CPU quota (`cpu_quota_cpus` in the gate-slots host file, or `$GATE_CPU_QUOTA_CPUS`, #1254). The rest has to be added by hand around every gate: `nice -n 19 ionice -c 3 env GATE_CPU_QUOTA_CPUS=8 VALIDATE_MAX_CPUS=8 GOMAXPROCS=8 python3 scripts/gate.py ...`. That works, because nice and IO class are inherited and dagrun's own scope takes --max-cpus and derives a MemoryMax. But it's easy to forget, and there's no explicit memory ceiling.

# Design

- New optional keys in the gate-slots host file (never the repository's gate.toml): `nice` (0-19), `io_class` (`idle` | `best-effort`), `memory_max` (a systemd size), alongside `cpu_quota_cpus`.
- gate.py applies them to its containment scope (`-p Nice=`, `-p IOSchedulingClass=`, `-p MemoryMax=`, or by exec'ing the gate command under nice/ionice when the scope can't carry exec properties), passes the CPU width to validate and dagrun as today, and passes the memory ceiling to dagrun's `--mem` so its own scope can't widen it.
- `gate.py status` and the gate ledger record the effective profile.
- Unset keys mean today's behaviour.

# Acceptance Criteria

- A fixture test (stubbed systemd-run) shows the scope arguments and child environment for a full profile, and for none.
- A real run on a host shows nice 19, idle IO, CPUQuota and MemoryMax on the gate's processes and cgroups.

# Notes

Numbered from devbig030's interim block (1400-1469).
